# ADR-0005: What happens to V1 features whose meaning changes without aria2

- **Status:** Accepted
- **Date:** 2026-10-22
- **Decided by:** the Product Owner
- **Issue:** #23

## Context

The parity map ([PARITY.md](../PARITY.md), #22) keeps every V1 feature by default. Some V1 settings and behaviours only existed because V1 handed transfers to aria2, a separate program with its own options. V2 has its own engine ([ADR-0001](0001-v2-direction.md)), so each of these needed a decision:

- **Performance profiles:** in V1 they chose aria2's `split`, `max-connection-per-server` and `min-split-size` (`optimizer.rs`). Changing connections mid-download restarted the transfer.
- **`advanced.file_allocation` and `advanced.disk_cache_mib`:** these are aria2 options.
- **Metalink:** aria2 handled it. libcurl disabled Metalink in curl 7.78.0, so V2 must build it itself.
- **Restart engine, engine diagnostics and session scrubbing:** these were features of the separate aria2 process.
- **`advanced.poll_interval_ms`:** V1 polled aria2 for progress.
- **The torrent-engine choice:** V1 could run torrents on aria2 or libtorrent.
- **`torrent.request_peer_speed_limit_kib`:** an aria2 option.

## Decision

| Item | Decision |
|---|---|
| Performance profiles (Conservative, Balanced, Maximum, Custom) | **Kept, as caps** for the adaptive engine: each profile sets the maximum connections and how eagerly V2 adds them. V2 still adapts within the caps |
| Custom profile fields | `max_connection_per_server` stays as the **per-host connection cap**, and `min_split_size_mib` as the **minimum segment size**. aria2's **`split` count is removed**, because V2 splits adaptively |
| `advanced.file_allocation` | **Mapped** to V2's preallocation choice: none, sparse or full. The default is chosen by measurement (#43; NTFS zero-fill, research report open question 7) |
| `advanced.disk_cache_mib` | **Mapped** to V2's writer buffer size (#49) |
| Metalink | **In the first release** (#54), keeping the opt-in setting `advanced.enable_metalink` |
| Restart engine and engine diagnostics | **"Restart transfer engine" stays if #38 makes the engine a helper process.** If the engine runs inside the app, it's replaced by an engine health display (#28) |
| `advanced.poll_interval_ms` | **Removed:** progress arrives as engine events (#58) |
| `performance.allow_mid_download_tuning` | **Removed:** V2 changes connections live without restarting, so tuning is always allowed within the profile caps (#47) |
| Torrent-engine choice (`torrent.engine`) and aria2-only torrent behaviours | **Dropped:** libtorrent is the only torrent engine. Every torrent feature stays: v1, v2-only, hybrid, magnets and seeding (#62) |
| `torrent.request_peer_speed_limit_kib` | **Kept if libtorrent supports it**, otherwise dropped with a note (checked in #62) |

### Follow-on consequences

These follow from the decisions above and from #38. They take effect unless the PO says otherwise in this ADR's review:

- **Process model:** behaviours of the separate engine process follow #38:
  - reconciling lost downloads becomes **resume from checkpoints**, with no aria2 GIDs (#43);
  - supervision and restart back-off (#39);
  - "engine missing";
  - `notifications.on_engine_failure`.
- **Session scrubbing** becomes the rule that **checkpoints never hold secrets** (#43, #71).
- **The aria2 optimizer** is replaced by the profile caps and adaptive budgets (#47, #57).

## Options considered

The options for each item are recorded in #23. The recommended option was chosen for each.

## Consequences

- **Upgrade notes:** removed settings (`poll_interval_ms`, `allow_mid_download_tuning`, `torrent.engine` and the Custom `split`) need a short note for users moving from V1 (#79). V2 never reads V1's settings unless #24 decides otherwise.
- **Settings model:** #30 builds the V2 settings with these mappings.
- **Profiles:** #57 maps the profiles to real engine controls.
- **Metalink:** #54 is first-release scope.
- **Parity map:** [PARITY.md](../PARITY.md) marks each affected row **Decided (ADR-0005)**.
