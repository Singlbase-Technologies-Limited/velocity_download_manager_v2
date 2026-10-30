# Building the native libraries

V2's direct-download engine uses libcurl ([ADR-0001](decisions/0001-v2-direction.md)). Release builds link a libcurl that V2 builds itself, from verified sources ([ADR-0004](decisions/0004-libcurl-build-path.md)), so the shipped version can be proven. This page covers how that build works, how to run it, and how to update a pin. The approval and pinning rules are in [DEPENDENCIES.md](DEPENDENCIES.md#native-libraries).

The torrent bridge uses libtorrent, with Boost and OpenSSL, built with vcpkg for now: see [libtorrent](#libtorrent).

## What gets built

| Target | Libraries, in build order | TLS | Build system |
|---|---|---|---|
| Windows x64 (`x86_64-pc-windows-msvc`) | zlib, nghttp2, curl | Schannel (the operating system's) | CMake with the Visual Studio generator |
| Linux x64 (`x86_64-unknown-linux-gnu`) | zlib, nghttp2, OpenSSL, curl | A static OpenSSL, on the 3.5 long-term line | Each project's own `configure` script |
| macOS | **Not established** (#83) | — | — |

The versions, sources and signing keys are in [`scripts/native/deps.lock`](../scripts/native/deps.lock). Every library is built static, so nothing from the build host's own libcurl is linked or bundled ([DEPENDENCIES.md](DEPENDENCIES.md#distribution-packages)).

**libcurl keeps only what V2 uses.** The build disables DICT, FILE, Gopher, IMAP, IPFS, LDAP and LDAPS, MQTT, POP3, RTSP, SMTP, Telnet, TFTP, WebSockets and the `.netrc` parser. Libraries V2 doesn't use stay out too: libpsl, libidn2, Brotli, zstd, libssh2, libssh, librtmp and GSS-API. A protocol that isn't compiled in can't be reached by any URL or redirect. The engine's own allowlists (#67) still apply on top. SFTP comes later, once its backend is decided (#52, #53).

## Running it

```sh
pnpm native:fetch   # download and verify the sources only
pnpm native:build   # fetch, build, check and write build-info.json
pnpm native:verify  # the release blocker: the record and every artefact still match
```

Both work on the Rust host target. The output goes to `target/native/` (ignored by git):

| Path | What it holds |
|---|---|
| `target/native/sources/` | The downloaded archives and their `.asc` signatures |
| `target/native/<target>/prefix/` | The built static libraries (`lib/`), headers (`include/`) and the curl tool (`bin/`) |
| `target/native/<target>/build-info.json` | The [build identity record](#build-infojson) |

**Work folder:** Linux compiles in `target/native/<target>/build/`. Windows compiles in a new temporary folder (`%TEMP%\v2n-…`), because MSVC fails under a deep checkout path: a clean clone 140 characters deep hit its 260-character limit (C1083, "Cannot open compiler generated file"). Both folders are deleted after the build, whether it passed or failed.

**Reuse:** a finished build is reused while the lock file and the scripts in `scripts/native/` are unchanged, and while every recorded artefact still has its recorded SHA-256. `node scripts/native/build.mjs --force` rebuilds anyway.

**Measured times** (cold, 2026-10-22):

| Platform | Machine | Time |
|---|---|---|
| Windows 11 | 16 logical CPUs, VS Build Tools 2022 (MSVC 19.44, CMake 3.31.6) | 150 s |
| Linux, Ubuntu 24.04 in WSL2 | 2 vCPUs, gcc 13.3.0, GNU Make 4.3 | 302 s |

### Prerequisites

| Platform | Needs | Notes |
|---|---|---|
| Windows | Visual Studio 2022 (or Build Tools) with "Desktop development with C++"; gpg | CMake comes with Visual Studio. gpg comes with Git for Windows (`C:\Program Files\Git\usr\bin\gpg.exe`), which the fetch script finds without it being on `PATH` |
| Linux | A C compiler, `make`, `perl` (for OpenSSL), `tar` with xz support, gpg | On Debian or Ubuntu: `sudo apt install build-essential perl xz-utils gnupg` |
| Both | Node.js and Rust, as for the rest of V2 ([DEVELOPMENT.md](DEVELOPMENT.md#prerequisites)) | `rustc -vV` gives the host target |

## How each source is proven

[`scripts/native/fetch.mjs`](../scripts/native/fetch.mjs) does this for every library the target needs, before anything is unpacked:

1. **Download** the archive over HTTPS from the URL in the lock file, into a `.part` file that is renamed only when complete.
2. **Hash:** compare the archive's SHA-256 with the lock file. On a mismatch, the file is deleted and the build stops.
3. **Signature:** download the detached `<url>.asc` and check it with gpg in a new, empty gpg home that holds **only** that library's key (`scripts/native/keys/<name>.asc`). It passes only when gpg reports exactly one valid signature whose **primary** key is the fingerprint in the lock file. A bad signature, a different key, an expired or revoked key, or two signatures stop the build.

The rules are in [`native-lib.mjs`](../scripts/native/native-lib.mjs), with unit tests beside it.

## Checking what was linked

After the build, [`build.mjs`](../scripts/native/build.mjs) runs the curl tool it just built (`curl -V`) and checks what is actually linked:

- **libcurl** is the version the lock file pins, and at least the **version floor, 8.23.0** (research report section 8.1).
- **nghttp2 and zlib** are the pinned versions.
- **TLS** is Schannel on Windows, and the pinned OpenSSL on Linux.
- **Protocols:** `http`, `https`, `ftp` and `ftps` are present, and nothing else is (SFTP is allowed once enabled).
- **Features:** `HTTP2`, `SSL` and `libz` are present.

Any mismatch fails the build, and no `build-info.json` is written.

Real output of both builds (2026-10-22):

```text
curl 8.23.0 (Windows) libcurl/8.23.0 Schannel zlib/1.3.2 nghttp2/1.70.0
Release-Date: 2026-10-14
Protocols: ftp ftps http https
Features: alt-svc AsynchDNS HSTS HTTP2 HTTPS-proxy IPv6 Kerberos Largefile libz SPNEGO SSL SSPI threadsafe UnixSockets

curl 8.23.0 (x86_64-pc-linux-gnu) libcurl/8.23.0 OpenSSL/3.5.9 zlib/1.3.2 nghttp2/1.70.0
Release-Date: 2026-10-14
Protocols: ftp ftps http https
Features: alt-svc AsynchDNS HSTS HTTP2 HTTPS-proxy IPv6 Largefile libz SSL threadsafe UnixSockets
```

On Linux, the built curl tool links only the C library dynamically (`ldd`: `linux-vdso`, `libc.so.6`, `ld-linux-x86-64`). A manual check on both builds fetched `https://curl.se/` over HTTP/2 with a verified certificate, and refused `https://expired.badssl.com/` (Windows: `SEC_E_CERT_EXPIRED`; Linux: OpenSSL verify result 10, "certificate has expired"). That was a one-off check against the public internet, not a test; TLS behaviour is tested in #69.

## Linking into Rust

`crates/velocity-curl` is the only crate that calls libcurl ([ADR-0002](decisions/0002-engine-crate-layout.md)). It links this build, never `curl-sys`'s bundled curl 8.22.0 or a system libcurl:

- **Overrides:** [`.cargo/config.toml`](../.cargo/config.toml) overrides the build scripts of `curl-sys` (`links = "curl"`) and `libz-sys` (`links = "z"`), so neither compiles a bundled copy nor looks for a system one ([build-script overrides](https://doc.rust-lang.org/cargo/reference/build-scripts.html#overriding-build-scripts)). `curl-sys` has no default features, so it pulls in no `openssl-sys`.
- **Linking:** `crates/velocity-curl/build.rs` reads `build-info.json` and links the static libraries in `target/native/<target>/prefix/lib`, plus the system libraries curl's own `libcurl.pc` lists (Windows: `bcrypt`, `advapi32`, `crypt32`, `secur32`, `ws2_32`, `iphlpapi`). On Linux it lists none; `pthread` and `dl` are added for a static OpenSSL on glibc before 2.34.
- **Refusal:** the build stops with a message naming `pnpm native:build` when the record is missing, is for another target, records a libcurl below the floor, or lists an artefact that is gone.
- **Location:** `.cargo/config.toml` sets `VELOCITY_NATIVE_DIR` to `target/native` as an absolute path. A `VELOCITY_NATIVE_DIR` already in the environment wins, for a build kept elsewhere.
- **Checks build it first:** `pnpm check:fast`, `check:pr` and `test:focused -- <crate>` run `native:build` before any Rust check. Plain `cargo` doesn't, and stops with the refusal message instead.

## Checking at run time

`velocity_curl::Libcurl::start()` is the only way to get a libcurl handle. It initialises libcurl once per process, reads `curl_version_info`, and returns `StartError::Incompatible` unless the linked libcurl passes the same checks as the build: version 8.23.0 or later, the platform's TLS backend (Schannel or OpenSSL), the required protocols and features, and no other protocol. The error lists every problem and names the fix:

```text
the transfer engine can't start: libcurl 8.22.0 is linked, but V2 needs 8.23.0 or newer. Rebuild the native libraries with `pnpm native:build` (docs/BUILDING_NATIVE.md)
```

The engine (#37) obtains libcurl only through `start()`, and maps a `StartError` to the `ENGINE_INCOMPATIBLE` error code for the UI. The tests: `crates/velocity-curl/src/capabilities.rs` (the check, against stand-in values), `src/runtime.rs` (`start` refuses a stand-in 8.22.0), and `tests/linked_libcurl.rs` (the real linked libcurl starts and reports exactly `ftp ftps http https`).

## build-info.json

One record per build, schema 1. It is written only after every check passes. `checkBuildInfo` in `native-lib.mjs` checks a record against the lock file. A missing or incomplete record is a release blocker ([DEPENDENCIES.md](DEPENDENCIES.md#release-blockers-related-to-provenance)).

**`pnpm native:verify`** enforces it. It fails unless the host target's record passes `checkBuildInfo`, and every artefact it lists still has its recorded SHA-256, so a library replaced after the build is caught. `pnpm tauri build` runs it first (`beforeBuildCommand` in `src-tauri/tauri.conf.json`), so no package is built without a complete record.

| Field | Content |
|---|---|
| `schema` | `1` |
| `target` | The Rust target triple |
| `v2` | The V2 commit and whether the working tree had uncommitted changes (`dirty`) |
| `toolchains` | `rustc -V`, the Node.js version, and the C compiler; also CMake (Windows) or `make` (Linux) |
| `cacheKey` | A SHA-256 over the target, the lock file and the build scripts |
| `libraries` | For each library: name, version, source URL, source SHA-256, signature result, licence, and the exact configure or CMake options |
| `linked` | The parsed `curl -V`: libcurl version, TLS backend, the other linked libraries, protocols and features |
| `artifacts` | The SHA-256 of every built static library and of the curl tool |

## Updating a pin

Each change to `deps.lock` is its own reviewed PR. A libcurl floor bump also needs the floor changed in `native-lib.mjs` (`LIBCURL_FLOOR`) and in the docs that state it.

1. **Check the advisories first:** curl's [vulnerability table](https://curl.se/docs/vulnerabilities.html) and the advisories for nghttp2, zlib and OpenSSL (#75).
2. **Download** the new archive and its `.asc` from the project's official release location.
3. **Verify the signature** with the key in `scripts/native/keys/`. If the project signs with a new key, get it from the project's own published source and record that source in the lock file's header.
4. **Update the line:** version, URL, SHA-256 and fingerprint. The parser refuses a source file name that doesn't match `<name>-<version>.`.
5. **Run `pnpm native:build`** on Windows and Linux. CI runs both native jobs for the PR, because the lock file changed ([CI.md](CI.md#selecting-jobs)).
6. **Record it:** the approval in [DEPENDENCIES.md](DEPENDENCIES.md#approved-direct-dependencies), and the measured times above if they changed.

## libtorrent

The torrent bridge (`crates/velocity-lt`) links libtorrent-rasterbar 2.0.15, Boost 1.91 and OpenSSL 3.5.9, built with [vcpkg](https://github.com/microsoft/vcpkg) (PO decisions on #62). `pnpm native:build` builds them after libcurl, with [`scripts/native/libtorrent.mjs`](../scripts/native/libtorrent.mjs).

- **The pins:**
  - **The registry:** [`scripts/native/vcpkg/vcpkg.json`](../scripts/native/vcpkg/vcpkg.json) pins the vcpkg registry by its `builtin-baseline` commit, `9e593bb` (2026-07-29). Boost and vcpkg's own helpers come from that commit.
  - **Overlay ports:** the registry offers libtorrent 2.0.11 and then 2.1.x, and OpenSSL 3.5 only up to 3.5.4. So libtorrent 2.0.15 and OpenSSL 3.5.9 come from overlay ports in [`scripts/native/vcpkg/ports`](../scripts/native/vcpkg/ports). Each is vcpkg's own recipe (libtorrent 2.0.11's, OpenSSL 3.5.4's) with only the version and the SHA-512 changed, and libtorrent's licence field corrected to its `LICENSE` file. The repository stores two of the OpenSSL port's files with LF where vcpkg has CRLF; its patches are byte for byte vcpkg's.
  - **Checksums:** vcpkg checks each source's SHA-512 on every download. Those archives are GitHub's archives of the release tags. V2 hasn't checked them against release signatures, as it does for libcurl's sources.
- **Release only:** the triplets are `x64-windows-static-md-release` and `x64-linux-release`, so only release libraries are built; cargo links those in every profile. libtorrent is built without iconv (`"default-features": false`).
- **The record:** the build writes `target/native/<target>/libtorrent-info.json`:
  - libtorrent's version and the vcpkg baseline;
  - the install prefix (`target/native/vcpkg-installed/<triplet>`);
  - the compile definitions and link libraries, read from libtorrent's CMake package. The shim must be compiled with the same definitions as libtorrent, such as `TORRENT_USE_OPENSSL` and `TORRENT_SSL_PEERS`;
  - the SHA-256 of each library.

  `crates/velocity-lt/build.rs` refuses to build without it, or when it records another libtorrent version. A generator expression the build doesn't know is refused rather than guessed at.
- **Reuse:** the build is reused while the manifest, the ports, `libtorrent.mjs` and `libtorrent-lib.mjs` are unchanged and every library still has its recorded SHA-256. vcpkg also keeps built packages in its own binary cache.
- **vcpkg itself:** `libtorrent.mjs` uses `VCPKG_ROOT` when it is set, else `C:\vcpkg` on Windows and `~/vcpkg` on Linux. The clone must contain the baseline commit. `pnpm check:prereqs` reports a missing vcpkg.
  - **Windows:** `git clone https://github.com/microsoft/vcpkg C:\vcpkg`, then `C:\vcpkg\bootstrap-vcpkg.bat -disableMetrics`.
  - **Linux:** `sudo apt install curl zip unzip tar`, then `git clone https://github.com/microsoft/vcpkg ~/vcpkg` and `~/vcpkg/bootstrap-vcpkg.sh -disableMetrics`. vcpkg downloads its own CMake and Ninja.
- **In CI** (#205): the native libraries are cached on their own, and vcpkg is set up at the baseline commit only when they have to be built ([CI.md](CI.md#caching)).
- **Memory:** compiling libtorrent takes up to about 1.3 GB per compiler process. On a 2 GB WSL the compiler was killed for lack of memory. Raise WSL's memory in `.wslconfig`, or set `VCPKG_MAX_CONCURRENCY` lower (this machine uses 4 with 6 GB).
- **Measured times** (cold, 2026-10-26; Boost and OpenSSL make up most of it):

  | Platform | Machine | Time |
  |---|---|---|
  | Windows 11 | 16 logical CPUs, VS Build Tools 2022; debug and release | 860 s |
  | Linux, Ubuntu 24.04 in WSL2 | 8 vCPUs, 6 GB, `VCPKG_MAX_CONCURRENCY=4`; release only | 1939 s |

  A clean Windows time for the release-only triplet wasn't measured: that run shared the CPU with other builds.
- **Not yet V2's own build:** releases are meant to link libraries V2 builds from verified sources, as for libcurl. Building libtorrent and Boost that way is a release blocker ([DEPENDENCIES.md](DEPENDENCIES.md#release-blockers-related-to-provenance)).

## Known limitations

- **Linux trust store:** the Linux build looks for CA certificates at `/etc/ssl/certs/ca-certificates.crt` and `/etc/ssl/certs`, where Debian, Ubuntu and Arch keep them. Fedora and openSUSE keep them elsewhere. How each TLS backend finds and checks certificates is tested in #69.
- **No IDN in libcurl:** libcurl doesn't convert international domain names (libidn2 is LGPL, and WinIDN would exist on Windows only). How V2 handles them is part of URL validation in #67.
- **Not in the app yet:** only `velocity-curl` links this build. The desktop app starts using it with the engine prototype (#37).
- **macOS:** not established (#83), for libcurl and libtorrent alike. Recorded as NOT RUN.
