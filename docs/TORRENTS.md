# Torrents

V2 downloads BitTorrent v1, v2 and hybrid torrents with libtorrent 2.0.15 ([ADR-0001](decisions/0001-v2-direction.md), [ADR-0005](decisions/0005-v1-features-without-aria2.md)). This document describes how torrents work in V2 today. #62 delivered the port in four parts: the bridge and helper, magnets and file choice, the lifecycle and seeding, and trackers and port mapping.

## Where the code is

| Part | Crate | Role |
|---|---|---|
| The bridge | [`crates/velocity-lt`](../crates/velocity-lt) | A small C++ shim (cxx) over one libtorrent session. The only crate that calls libtorrent ([ADR-0002](decisions/0002-engine-crate-layout.md)). It never deletes files |
| The torrent engine and helper | [`crates/velocity-torrent`](../crates/velocity-torrent) | What reaches libtorrent and what its state means; the helper program `velocity-torrent serve`, its client and its supervisor |
| Parsing and checks | [`crates/velocity-core/src/torrent.rs`](../crates/velocity-core/src/torrent.rs) | `.torrent` files, magnet links and resume data, checked before libtorrent sees them |
| Seeding rules | [`crates/velocity-core/src/seeding.rs`](../crates/velocity-core/src/seeding.rs) | Seeding policies, when seeding ends, and the seeding queue |
| Deleting files | [`crates/velocity-core/src/torrent_files.rs`](../crates/velocity-core/src/torrent_files.rs) | Which files a torrent owns, and deleting only those |
| The contract | [`crates/velocity-core/src/torrent_engine.rs`](../crates/velocity-core/src/torrent_engine.rs) | `TorrentEngine`, the commands, replies and events |
| Torrents in the manager | [`crates/velocity-core/src/manager/torrents.rs`](../crates/velocity-core/src/manager/torrents.rs) | Adding, queueing, storing and restoring torrents, and deleting their files ([In the app](#in-the-app-58)) |

libtorrent runs in the **torrent helper**, a child process the app starts and supervises. A crash in libtorrent therefore ends only the helper ([ADR-0014](decisions/0014-torrent-helper-child-process.md)). The helper speaks the same protocol as the direct-download engine, and the same supervisor restarts it ([ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md)). The app starts it from beside its own executable (#58). The app depends on `velocity-torrent` without its `engine` feature, so it gets the helper's client and supervisor and never links libtorrent; only the helper program turns the feature on (PO decision on #58).

How libtorrent, Boost and OpenSSL are built is in [BUILDING_NATIVE.md](BUILDING_NATIVE.md#libtorrent).

## Checks before libtorrent

Torrent metadata and magnet links are untrusted input. `velocity_core::torrent` checks them, and the helper checks them again before calling libtorrent.

- **Bounds:**
  - **The file:** at most 16 MiB, read only up to that size.
  - **The structure:** at most 100,000 files, 64 levels of nesting, 2,000,000 bencode entries, and 64 path parts per file.
  - **Trackers:** at most 64, each up to 2,048 bytes.
  - **Errors:** the parser never panics on malformed input.
- **Hashes:**
  - **Info hashes:** the v1 hash (SHA-1) and the v2 hash (SHA-256) are computed over the info dictionary's exact bytes. A v2-only torrent never gets a made-up v1 hash.
  - **v2 piece layers:** each must match its file's Merkle root.
  - **Hybrids:** a hybrid's v1 and v2 file lists must describe the same files.
- **Names:**
  - **The rule:** every file and folder name follows `naming::check_file_name_in_folder`. That is V2's file-name rule, except that a name may start with a dot, such as `.gitignore`, `.nfo` or a hybrid's `.pad` entries (PO decision on #62).
  - **What it refuses:** `.` and `..`, separators, control characters, Windows device names, and names Windows can't store (`? * < > |`, a trailing dot). Refusing them means the names on disk are always the torrent's own. V1 refused fewer names.
  - **Duplicates:** two paths that differ only in case, or one name used for both a file and a folder, are refused.
- **Magnets:**
  - **Hashes:** v1 (`urn:btih:`, 40 hexadecimal or 32 base32 characters) and v2 (`urn:btmh:1220…`) are accepted. Two different hashes of one kind are refused.
  - **What reaches libtorrent:** only the canonical form: the hashes, the display name and the trackers. Other parameters, such as `x.pe` peer addresses, are dropped.
- **Trackers:** `http`, `https` or `udp` with a host, and no spaces or control characters. Others are dropped from a torrent, and refused when given with `add`.
- **Folders:** a torrent's folder must be an existing absolute folder, not a link.
- **A magnet's metadata** comes from peers. Every magnet is added wanting nothing, so libtorrent fetches its metadata and writes no file. When the metadata arrives, the helper checks it with the core's `parse_metadata`, with every rule above:
  - **If it passes:** the metadata-only torrent is removed and added again from the checked metadata, as an ordinary torrent with its files wanted, as V1 did. The torrent then waits in Choosing files if the user is to choose them.
  - **If it fails:** the torrent fails with the reason, before any byte of it is written. libtorrent alone would rename unsafe names and download anyway.

## File choice

- **When:** a torrent added with `chooseFiles` downloads nothing and waits in **Choosing files**: a magnet once its metadata is checked, a `.torrent` file at once. A `.torrent` file can instead be added with its first choice in `files`.
- **Choosing:** `setFiles` names the files to download by their index in the torrent (`TorrentFile::index`, libtorrent's index plus one). Padding files can never be chosen. An empty choice, a duplicate or an index the torrent hasn't is refused (`INVALID_INPUT`), and so is a choice before the metadata is known (`INVALID_STATE`).
- **Changing the choice:** choosing again adds or drops files while the torrent downloads. A completed torrent that gets more files downloads them and completes again. Dropping a file never deletes what is on disk.
- **Peers:** a torrent that wanted nothing, or completed and stopped, has dropped its peers. Leaving Choosing files, or getting more files after completing, removes it from the session and adds it again with the new choice, so it finds peers as a new torrent does, and libtorrent checks the files already on disk. In the tests, without this a torrent waited longer than 120 s for peers.
- **Libtorrent's part file:** in a v1 or hybrid torrent a piece can span two files. When only one of them is chosen, libtorrent keeps the other file's part of that piece in a file of its own, `.<info hash>.parts`, in the torrent's folder, never in the unchosen file.
- **Progress:** with only some files chosen, progress is the chosen files' bytes, from libtorrent's per-file progress. libtorrent's own count is in whole pieces.
- **The metadata:** `metadata` returns a torrent's metadata (`d4:info…e`) in chunks of at most 1 MiB, as hex, because metadata may be up to 16 MiB and a message at most 4 MiB. The manager joins the chunks once the metadata is known, checks them with `parse_metadata` and against the info hash the helper reports, and keeps them ([In the app](#in-the-app-58)). The resume data holds the metadata too ([Resume data](#resume-data-and-recovery)).

## The torrent contract

`TORRENT_CONTRACT_VERSION` is 4: version 1 had no file choice, version 2 no seeding or resume data, and version 3 no trackers, peers, limits or port settings. Every wire form refuses unknown fields.

| Command | Reply | What it does |
|---|---|---|
| `version {}` | `version { libtorrent }` | The libtorrent version, for diagnostics; also the supervisor's hang check |
| `setSettings { settings }` | `done {}` | [The settings](#settings-and-identity) |
| `add { spec }` | `added { id }` | `spec`: `source` (`{"kind":"file","path":…}`, `{"kind":"magnet","uri":…}` or `{"kind":"resume"}`), `directory`, `trackers`, `paused`, `chooseFiles`, `files` (a `.torrent` file's first choice, or `null` for every file), `seeding` (its [policy](#seeding)), and `uploadLimit` and `downloadLimit` (bytes per second, 0 for none) |
| `restore { id, spec }` | `done {}` | Adds a torrent under an id an earlier helper gave it ([Recovery](#resume-data-and-recovery)). `INVALID_INPUT` for an id the helper has, or could still give |
| `setFiles { id, files }` | `done {}` | Downloads exactly these files ([File choice](#file-choice)) |
| `setSeeding { id, seeding }` | `done {}` | Sets when the torrent stops seeding; `{"mode":"never"}` stops it ([Seeding](#seeding)) |
| `metadata { id, offset }` | `metadata { chunk }` | `chunk`: `total`, `offset`, `hex`: at most 1 MiB of the metadata from `offset`; `INVALID_STATE` before it is known |
| `upload { chunk }` | `done {}` | Part of the resume data the next `{"kind":"resume"}` source adds from, in order from offset 0 ([Resume data](#resume-data-and-recovery)) |
| `saveResume {}` | `saved { ids }` | Asks for every torrent's resume data now; it follows as `resumeData` events |
| `pause { id }`, `resume { id }` | `done {}` | `INVALID_STATE` for a completed torrent that doesn't seed |
| `remove { id }` | `done {}` | Forgets the torrent. Its files are always kept |
| `setLimits { id, upload, download }` | `done {}` | The torrent's own limits, in bytes per second, 0 for none |
| `trackers { id }` | `trackers { trackers }` | Each tracker: its URL redacted, tier, state (`working`, `updating`, `error` or `notContacted`), message, failures, seconds to the next announce, and the seeds and peers it counted ([Trackers and peers](#trackers-and-peers)) |
| `reannounce { id }` | `done {}` | Announces to the torrent's trackers now |
| `peers { id }` | `peers { peers }` | Each connected peer: address, port, speeds, whether it is a seed, its progress in thousandths, and its client |
| `network {}` | `network { network }` | The listening port, a listen error, and the port-mapping state ([Settings](#settings-and-identity)) |
| `snapshot {}` | `snapshot { torrents }` | The state of every torrent |

- **Events:** `changed { snapshot }`, `removed { id }`, and `resumeData { id, chunk }`, the torrent's resume data in chunks sent in order.
- **A snapshot** also carries `uploadedBytes` and `seedingSecs`, over the torrent's life, and `seedQueued`.
- **A `.torrent` file by path:** the helper reads the file itself. A large one would not fit the protocol's 4 MiB message limit.
- **Errors:**
  - **The input:** `NOT_FOUND` for an unknown id or a missing file, `INVALID_INPUT` for metadata that doesn't pass the checks, `INVALID_FILENAME` for an unsafe name, `INVALID_URL` for a bad link or tracker, `DESTINATION_MISSING` or `INVALID_PATH` for the folder.
  - **Duplicates:** `FILE_EXISTS` for a torrent the helper already has.

### Statuses

The helper reads libtorrent's state every 250 ms (a starting value) and sends `changed` when a torrent's snapshot changes.

| libtorrent | V2 status |
|---|---|
| An error | Failed, with libtorrent's message (not retryable) |
| Paused | Paused |
| Checking files or resume data | Verifying |
| A magnet whose metadata isn't known and checked yet | Fetching metadata |
| Files known, none chosen yet | Choosing files |
| Metadata that failed V2's check | Failed, with the reason (not retryable) |
| Downloading | Downloading |
| Finished, being flushed | Verifying |
| Flushed and stopped | Completed |
| Flushed, and seeding under its policy | Seeding |
| Flushed, and waiting for a seeding slot | Completed, with `seedQueued` |
| Flushed, and paused by the user while it may seed | Paused |

- **Completion:** a torrent is Completed only after every wanted piece has passed its hash check and libtorrent has flushed its files to disk. It then stops, and seeds only under a policy ([Seeding](#seeding)).
- **Speeds:** a stopped completed torrent shows no speed. libtorrent's averaged rates take a while to fall, so the helper reports 0 instead.
- **Progress:** it counts the bytes of the wanted files. Padding is never counted, and the size is unknown until the files are chosen.
- **A crashed helper:** the torrents it had fail with a retryable error, then come back ([Recovery](#resume-data-and-recovery)).

## Seeding

Seeding is off by default (ADR-0005). The policies and their limits are V1's (PO decisions on #62), and the torrent helper applies them.

- **Policies** (`seeding` in `add`, and `setSeeding`):
  - `never`: stop once complete. The default.
  - `time` (`minutes`, 1 to 525,600), `ratio` (`ratio`, 0.1 to 100), or `timeOrRatio` (both): stop at the first limit reached.
  - `unlimited`: seed until the user stops it.
  - Anything outside these ranges is refused with `INVALID_INPUT`.
- **What the limits count:** libtorrent's own seeding time and upload total over the torrent's life. The ratio is uploaded bytes over the bytes of the files downloaded. Both come back with the resume data, so a restart doesn't reset them, and a policy already met doesn't seed again.
- **The seeding queue:**
  - **The limit:** at most `maxActiveSeeds` torrents seed at once; 0, the default, means no limit, and more than 1,000 is refused (V1's limits).
  - **The order:** the torrent waiting longest starts first. When the limit is lowered, the torrents that started seeding last go back to the queue.
  - **Waiting:** a torrent in the queue is stopped, and shows as Completed with `seedQueued`.
- **Controls:**
  - `pause` and `resume` work on a seeding torrent: a resumed one goes back through the queue.
  - `setSeeding` with `never` stops it.
  - A new policy that isn't met yet makes a completed torrent seed again.
  - A torrent added paused, or paused by the user, stays paused when it completes.

## Resume data and recovery

Resume data is libtorrent's own record of a torrent: its metadata, which pieces it has, its file choice, its trackers and its seeding totals. It isn't a [checkpoint](GLOSSARY.md); the direct-download engine's checkpoints are V2's own format.

- **When the helper sends it:** as `resumeData` events in chunks of at most 1 MiB, in order:
  - when libtorrent says it changed, at most every 10 s (a starting value: a crash loses at most that much of the record);
  - when the torrent completes;
  - for `saveResume`.
- **What isn't sent:** a magnet whose metadata isn't known yet has none; it is added again by its link.
- **Adding a torrent back:** `upload` sends the resume data in chunks, then `add` or `restore` with `{"kind":"resume"}` adds the torrent from it.
  - **Checks first:** the resume data comes back from storage, so `check_resume_data` checks it before libtorrent reads it:
    - only the entries libtorrent 2.0.15 writes are accepted. Never `mapped_files`, which would rename the torrent's files;
    - the metadata inside it passes `parse_metadata`, and its info hashes must match that metadata;
    - trackers and web seeds pass the tracker checks, and web seeds must be `http` or `https`;
    - the per-file priorities must match the files;
    - modes V2 never sets are refused: seed mode, share mode, super-seeding and I2P;
    - at most 32 MiB.
  - **What libtorrent gets:** the metadata V2 checked, not the copy inside the resume data.
  - **What libtorrent trusts:** the resume data's record of which pieces are done. libtorrent doesn't check the files again, as with V1.
  - **What the spec sets:** its folder, trackers, pause and policy apply. Its `files` changes the choice; without it, the resume data's choice stands. A torrent whose resume data chose nothing waits in Choosing files.
- **Where it is kept:** the supervisor keeps each torrent's latest resume data in memory, and the manager stores it in the database without its trackers ([In the app](#in-the-app-58)).
  - **Before the app closes:** the manager asks the helper for every torrent's resume data (`save_resume`) and waits up to 5 s for each fresh copy (`Manager::prepare_to_close`). The supervisor's `collect_resume_data` does the same for a caller without a manager.
- **A crashed helper:**
  - **First:** its torrents fail with a retryable error.
  - **Then:** the new helper gets each one back under its own id (`restore`): from its latest resume data, otherwise from its link or file. The choice, pause and policy set since apply. Commands for it work again.
  - **If one can't be added back:** it stays failed.
  - **The same happens** after a hang or a restart by hand (PO decision on #62).

## Deleting a torrent's files

The helper never deletes files. Deleting is the manager's decision, through `velocity_core::torrent_files` (PO decision on #62): `Manager::remove` with its files deletes, with the torrent's stored metadata, the files ever chosen, libtorrent's part file and the folders left empty.

- **What is deleted:**
  - the files ever chosen;
  - libtorrent's part file, `.<info hash>.parts`;
  - then each folder of the torrent that is left empty.

  Unchosen files are never deleted: libtorrent never writes them, so a file there of the same name is someone else's.
- **Only what is verifiably the torrent's:**
  - a plain file, no larger than the torrent says;
  - inside the torrent's folder, reached through plain folders.
  - A link or reparse point, such as a junction, is never followed, and neither is anything behind one. The torrent's folder itself must be a plain folder.
- **Windows:** a file still open can't be deleted, so each file is tried again for up to 5 s, after the helper has removed the torrent. On the development machine, libtorrent 2.0.15 had let go of a seeding torrent's files at once: the integration test passed without the wait three times out of three. A core unit test covers the wait with a file held open without delete sharing.
- **The report:** `delete_owned` says what it deleted and what it kept, and why: a link, too large, behind a link, a folder not empty, or the error.

## Trackers and peers

- **Tracker URLs are redacted** before they leave the helper: scheme, host and port only, as V1 showed them (`torrent::redact_tracker`). A private tracker's passkey, in the path or the query, and any login never reach the app's logs or UI.
- **Untrusted text is cleaned:** a tracker's message loses control characters and the tracker's own URL, and is cut to 300 characters. A peer's client name loses control characters and is cut to 64.
- **Trackers on localhost:** libtorrent refuses to announce to them unless the path starts with `/announce` (its SSRF mitigation). Trackers elsewhere aren't affected.

## Settings and identity

The Settings screen's BitTorrent section edits them ([SETTINGS.md](SETTINGS.md#advanced-and-new-torrents), #30). A new helper starts with every kind of peer discovery off and listens nowhere in particular, and the supervisor sends the settings right after the handshake. The defaults are V1's (PO decisions on #62), apart from the port:

| Setting | Default | Limits | What it does |
|---|---|---|---|
| `dht`, `peerExchange`, `localPeerDiscovery` | on, on, off | | How peers are found besides trackers. Private torrents never use DHT or peer exchange, which libtorrent enforces |
| `maxActiveSeeds` | 0 | 0 to 1,000 | [Seeding](#seeding) |
| `listenPort` | 6890 | 0, or 1,024 to 65,535 | The TCP and UDP port on every interface. V1's 6881 and 6882 are never reused (PO decision on #62). 0 picks one free port when it is set, the same on every interface; libtorrent given 0 itself would pick one per interface |
| `portMapping` | off | | Asks the router to forward the port, by UPnP and NAT-PMP |
| `uploadLimit`, `downloadLimit` | 0 | 0 to 2,147,483,647 bytes per second | All torrents together; 0 for none. Separate from the direct downloads' limit (PO decision on #62) |
| `extraTrackers` | none | at most 64, each a valid tracker | Added to every torrent added from then on |
| `maxPeersPerTorrent` | 55 | 1 to 512 | Peers each torrent added from then on connects to at most |
| `dhtBootstrapNodes` | none | at most 16, each `host:port` | Where DHT starts from (V1's `dht_entry_point`); none means libtorrent's own default nodes |

- **Local peers:** libtorrent doesn't apply the global limits to peers on the local network, as with V1. A torrent's own limits apply to every peer.
- **A port that is taken:** libtorrent moves on to the next free port, as with V1, and `network` says so: the port really used, and "port 6890 couldn't be opened; listening on 6891" (PO decision on #62). Port mapping maps the port really used.
  - **Windows:** another program's socket that didn't ask for exclusive use can share the port with libtorrent's. The helper then listens on it, with nothing to report.
- **Listen errors:** an interface libtorrent can't listen on is reported by `network` too. It stays reported until the port changes, even if another interface works, since peers on the failed one can't connect.
- **Port mapping:**
  - **The states** are V1's: `off`, `searching`, `mapped` (the external port, and `upnp` or `natpmp`), `failed` (both protocols, with their reasons), and `noRouter` when nothing answered within 30 s.
  - **A later failure** never hides a working mapping.
  - **What resets it:** turning it on, or changing the port, starts a new search.
- **V1's aria2 network settings:**
  - **`listen_port` and `dht_listen_port`:** these were aria2's two ports. libtorrent uses `listenPort` for TCP, uTP and DHT alike.
  - **`enable_dht6`:** aria2's too. libtorrent runs DHT on every listening socket, IPv4 and IPv6 together, under `dht`, as V1's libtorrent engine did.
- **Not kept from V1:** `request_peer_speed_limit_kib` (aria2's "look for more peers while slower than this") is dropped, as ADR-0005 planned for a setting libtorrent can't do. libtorrent 2.0.15 has no such setting; it finds peers on its own within its connection limits.
- **Identity:** peers see the peer-id prefix `-VD0100-` (`-VD` and the version digits), and trackers see `Velocity V2/0.1.0 libtorrent/2.0.15.0` (PO decision on #62). V1's `-VL` prefix isn't reused.

## In the app (#58)

The manager runs torrents beside direct downloads, through the `TorrentEngine` contract (`manager/torrents.rs`).

- **Adding:** `Manager::add_torrent` takes a `.torrent` file's absolute path or a magnet link, the folder, extra trackers, the file choice, the seeding policy and the limits. It checks them first: the magnet with `parse_magnet`, each tracker with `check_tracker`, and the policy and limits against their bounds. Nothing is stored when one is refused. The Add dialog (#60) adds through `Manager::add_request`: duplicates by info hash, the category from the largest file, and `.torrent` links fetched into the app's `torrent-links` folder and kept there without their trackers ([CONTRACTS.md](CONTRACTS.md#adding-downloads)).
- **The queue** (PO decision on #58): a torrent takes a slot of the queue while it fetches its metadata or downloads, as in V1, where aria2 counted torrents. Choosing files, seeding and paused don't take one; seeding has its own limit, `maxActiveSeeds`.
- **What is stored** (the `torrents` table, [DATA_AND_MIGRATIONS.md](DATA_AND_MIGRATIONS.md)):
  - where it came from: the `.torrent` file's path, or the magnet link without its trackers;
  - the file choice, the seeding policy and the limits;
  - its metadata, once known and checked;
  - its resume data, without its trackers (`torrent::strip_trackers`), saved whenever the helper sends it;
  - its info hashes and upload, on its `downloads` row.
- **Tracker passkeys** (PO decision on #58): tracker addresses can carry a private tracker's passkey, so none is stored. Trackers given this session stay in memory. Keeping them in the OS keychain is #70; until then a private torrent comes back without its trackers after the app restarts, and finds peers only through DHT or the settings' extra trackers.
- **Starting again:** a torrent comes back from its stored resume data (uploaded in chunks, then added with `{"kind":"resume"}`), and otherwise from where it came from. A completed torrent isn't added again; a seeding one is.
- **A helper that stops:** its supervisor adds the torrents back under the same ids. A torrent it reports lost (`torrent_engine::lost_by_helper`) shows as waiting, with the reason, and keeps its slot; the manager doesn't add it again too.
- **Commands:** pause, resume, cancel (the helper forgets it and its files are kept, as in V1), retry, remove with or without its files, the download limit, `choose_files` and `set_seeding`. A torrent's connections are libtorrent's, so `set_download_connections` is `UNSUPPORTED` for one.
- **What the window asks for while it shows a torrent** (#63, [CONTRACTS.md](CONTRACTS.md#the-downloads-commands)): its details from the stored metadata (`Manager::torrent_details`); its trackers and peers from the helper, every second, only while shown (PO decision on #63); re-announcing; the upload limit; and changing its files, which deletes the files dropped only when the user asks, and only the torrent's own (`delete_owned`).
- **Adding with choices** (#63): a `.torrent` file is previewed first (`Manager::preview_torrent`), with the download it would repeat. The Add request carries the files chosen for it, the seeding policy and the upload limit. A choice made before adding replaces "Choose files before downloading".
- **A late status:** a running status the helper sent before it paused a torrent the user paused doesn't undo the pause (#63, V1 #40), as for direct downloads (#59).
- **Without a helper:** when the program can't start, the app runs with `MissingTorrentEngine`: torrents wait, and the snapshot's `torrentEngine` says why.

## Not yet

- **Keeping trackers across a restart** (#70).

## Tests

| What | Where |
|---|---|
| Real transfers between two libtorrent sessions on loopback, for v1, v2-only and hybrid, single and multi-file; a v2 magnet's metadata; bad input and duplicates | `velocity-lt` `tests/transfer.rs` (V1's 5 bridge tests) |
| Parsing, hashes, piece layers, names, limits, magnets, trackers, against hand-built torrents and the corpus | `velocity-core` `torrent.rs` unit tests |
| How libtorrent states map to V2 statuses; the folder and file checks; the peer-id prefix | `velocity-torrent` `engine.rs` unit tests |
| The helper as a child process: a v1, a v2-only and a hybrid torrent and a v2 magnet downloaded from a seeder found through a loopback tracker; refused input; pause, resume and remove; a crash and its restart | `velocity-torrent` `tests/helper.rs` |
| File choice through the helper: a magnet waiting in Choosing files without writing anything, its metadata in chunks, only the chosen files downloaded and one more after completing; a `.torrent` file's first choice; unsafe metadata from a peer failing the torrent before any write; every refused choice | `velocity-torrent` `tests/helper.rs` |
| A crashed helper's torrents failing, then coming back under their ids | `velocity-torrent` `tests/helper.rs` |
| Seeding through the helper, with libtorrent sessions on loopback as downloaders: stopping at a share ratio; the seeding queue and a lowered limit; pausing, resuming and stopping unlimited seeding; refused policies and limits | `velocity-torrent` `tests/lifecycle.rs` (V1's seeding tests) |
| Resume data through the helper: a new helper continuing a torrent with its choice and without a duplicate; a paused torrent staying paused; seeding totals surviving; a magnet coming back with its metadata; a crashed helper's seeding torrent coming back; refused uploads and resume data; ids never given twice | `velocity-torrent` `tests/lifecycle.rs` (V1's restart and crash tests) |
| An active torrent's files deleted once the helper lets go, and nothing else | `velocity-torrent` `tests/lifecycle.rs` |
| Policies, their limits, when seeding ends, and the seeding queue | `velocity-core` `seeding.rs` unit tests (V1's policy and queue tests) |
| Resume data checks, each refusal for its own reason | `velocity-core` `torrent.rs` unit tests |
| Owned paths, and deleting only them: links, junctions, sizes, folders left with other files, and waiting for an open file on Windows | `velocity-core` `torrent_files.rs` unit tests (V1's owned-paths and deletion tests) |
| Resume data put back together from its chunks | `velocity-torrent` `supervisor.rs` unit tests |
| A torrent's download and upload limits holding back a peer, and the global limits not holding back a local one; a failing tracker shown redacted with its reason, and a re-announce recovering; the peers list; extra trackers; the listening port | `velocity-torrent` `tests/network.rs` (V1's limits and tracker tests). A taken port is checked on Linux |
| The session's settings; trackers and peers shown, redacted and cleaned; one free port for a random one | `velocity-torrent` `engine.rs` unit tests |
| The port-mapping state | `velocity-torrent` `portmap.rs` unit tests (V1's) |
| Tracker URLs redacted | `velocity-core` `torrent.rs` unit tests |
| Trackers taken out of resume data before it is stored | `velocity-core` `torrent.rs` unit tests |
| The manager's torrents against a fake helper: adding, the queue's slots, events, metadata and resume data stored, file choice, policy and limits, pause, resume and cancel, a lost torrent waiting, restoring from resume data or the origin, deleting files, closing, no helper | `velocity-core` `manager/tests.rs` (`torrents`) |
| The manager with a real helper: a torrent downloaded with its metadata and resume data kept without trackers; seeding again from resume data after the app starts again; back on its own after the helper is killed; removed with only its own files | `velocity-torrent` `tests/manager.rs` |

- **Isolation:** the tests never leave the machine. DHT, peer exchange, local peer discovery and port mapping are off, and peers are found through a tracker on 127.0.0.1, or connect to the helper's port. Port mapping is tested by its unit tests and the session's settings only: turned on, it would send UPnP and NAT-PMP requests to the network. For seeding, the test tracker answers each peer with every other peer that announced to it; never with the peer itself, since libtorrent keeps one entry per address by default and every peer is on 127.0.0.1.
- **The corpus:** [`tests/fixtures/torrents`](../tests/fixtures/torrents) comes from V1. [`scripts/make-torrent-corpus.py`](../scripts/make-torrent-corpus.py) generated it with libtorrent's own torrent creator (2.0.10, through its Python bindings). Running the script again needs those bindings; the committed files don't.
