# Torrents

V2 downloads BitTorrent v1, v2 and hybrid torrents with libtorrent 2.0.15 ([ADR-0001](decisions/0001-v2-direction.md), [ADR-0005](decisions/0005-v1-features-without-aria2.md)). This document describes how torrents work in V2 today. #62 delivers the port in parts: this is its first part.

## Where the code is

| Part | Crate | Role |
|---|---|---|
| The bridge | [`crates/velocity-lt`](../crates/velocity-lt) | A small C++ shim (cxx) over one libtorrent session. The only crate that calls libtorrent ([ADR-0002](decisions/0002-engine-crate-layout.md)). It never deletes files |
| The torrent engine and helper | [`crates/velocity-torrent`](../crates/velocity-torrent) | What reaches libtorrent and what its state means; the helper program `velocity-torrent serve`, its client and its supervisor |
| Parsing and checks | [`crates/velocity-core/src/torrent.rs`](../crates/velocity-core/src/torrent.rs) | `.torrent` files and magnet links, checked before libtorrent sees them |
| The contract | [`crates/velocity-core/src/torrent_engine.rs`](../crates/velocity-core/src/torrent_engine.rs) | `TorrentEngine`, the commands, replies and events |

libtorrent runs in the **torrent helper**, a child process the app starts and supervises. A crash in libtorrent therefore ends only the helper ([ADR-0014](decisions/0014-torrent-helper-child-process.md)). The helper speaks the same protocol as the direct-download engine, and the same supervisor restarts it ([ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md)). The app doesn't start the helper yet; #58 wires it in.

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

## The torrent contract

`TORRENT_CONTRACT_VERSION` is 1. Every wire form refuses unknown fields.

| Command | Reply | What it does |
|---|---|---|
| `version {}` | `version { libtorrent }` | The libtorrent version, for diagnostics; also the supervisor's hang check |
| `setSettings { settings }` | `done {}` | DHT, peer exchange and local peer discovery |
| `add { spec }` | `added { id }` | `spec`: `source` (`{"kind":"file","path":…}` or `{"kind":"magnet","uri":…}`), `directory`, `trackers`, `paused` |
| `pause { id }`, `resume { id }` | `done {}` | `INVALID_STATE` for a completed torrent |
| `remove { id }` | `done {}` | Forgets the torrent. Its files are always kept |
| `snapshot {}` | `snapshot { torrents }` | The state of every torrent |

- **Events:** `changed { snapshot }` and `removed { id }`.
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
| Downloading metadata (a magnet) | Fetching metadata |
| Downloading | Downloading |
| Finished, being flushed | Verifying |
| Flushed and stopped | Completed |

- **Completion:** a torrent is Completed only after every wanted piece has passed its hash check and libtorrent has flushed its files to disk. It then stops uploading, because seeding is off by default (ADR-0005).
- **Progress:** it counts the bytes of the wanted files. Padding is never counted.
- **A crashed helper:** the torrents it had fail with a retryable error. Their files stay.

## Settings and identity

- **Peer discovery:** the defaults are V1's (PO decision on #62): DHT on, peer exchange on, local peer discovery off. Private torrents never use DHT or peer exchange, which libtorrent enforces. A new helper starts with everything off, and the supervisor sends the settings right after the handshake. The settings screen is #30.
- **Port mapping:** UPnP and NAT-PMP are off. They come as an opt-in with #62's port-mapping part.
- **Listening:** on a random port on every interface, until that part adds a fixed port.
- **Identity:** peers see the peer-id prefix `-VD0100-` (`-VD` and the version digits), and trackers see `Velocity V2/0.1.0 libtorrent/2.0.15.0` (PO decision on #62). V1's `-VL` prefix isn't reused.

## Not yet

These come with #62's later parts:

- **Magnets and file choice:**
  - a metadata-only step, file choice and file priorities;
  - storing a magnet's metadata.
- **Lifecycle and seeding:**
  - resume data, and recovery after a restart;
  - seeding policies and their queue;
  - Windows file deletion rules, and deleting a torrent's files.
- **Trackers and port mapping:**
  - the tracker list and re-announce;
  - upload limits and per-peer limits;
  - opt-in port mapping and a fixed listening port.

## Tests

| What | Where |
|---|---|
| Real transfers between two libtorrent sessions on loopback, for v1, v2-only and hybrid, single and multi-file; a v2 magnet's metadata; bad input and duplicates | `velocity-lt` `tests/transfer.rs` (V1's 5 bridge tests) |
| Parsing, hashes, piece layers, names, limits, magnets, trackers, against hand-built torrents and the corpus | `velocity-core` `torrent.rs` unit tests |
| How libtorrent states map to V2 statuses; the folder and file checks; the peer-id prefix | `velocity-torrent` `engine.rs` unit tests |
| The helper as a child process: a v1, a v2-only and a hybrid torrent and a v2 magnet downloaded from a seeder found through a loopback tracker; refused input; pause, resume and remove; a crash and its restart | `velocity-torrent` `tests/helper.rs` |

- **Isolation:** the tests never leave the machine. DHT, peer exchange and local peer discovery are off, and peers are found through a tracker on 127.0.0.1.
- **The corpus:** [`tests/fixtures/torrents`](../tests/fixtures/torrents) comes from V1. [`scripts/make-torrent-corpus.py`](../scripts/make-torrent-corpus.py) generated it with libtorrent's own torrent creator (2.0.10, through its Python bindings). Running the script again needs those bindings; the committed files don't.
