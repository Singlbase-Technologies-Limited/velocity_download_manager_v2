# Dependency policy

Every third-party component V2 builds with or ships is approved, pinned, verified, recorded and credited in its notices. This page is the policy (#9). What the licences require is assessed in [LICENSING.md](LICENSING.md).

The rule in [AGENTS.md](../AGENTS.md#engineering) is short: **a new crate, npm package, native library, service or background process needs Product Owner approval.** This page says what that approval needs, and what happens afterwards.

## What counts as a dependency

| Kind | Examples | Pinned by |
|---|---|---|
| Rust crates | `tauri`, `serde`; later `curl`, `curl-sys` | `Cargo.toml` (direct) and the committed `Cargo.lock` (everything) |
| npm packages, installed with pnpm | `react`, `@tauri-apps/api`, `vite`, `vitest` | `package.json` (direct) and the committed `pnpm-lock.yaml`; CI uses `pnpm install --frozen-lockfile` |
| Native libraries, linked or bundled | libcurl, nghttp2, zlib, a TLS library, possibly libssh2, libtorrent, Boost, OpenSSL | The native lock file [`scripts/native/deps.lock`](../scripts/native/deps.lock) (below; #35) |
| Toolchains | Rust 1.99.1, Node.js 24.12.0, pnpm 12.14.0 | `rust-toolchain.toml`, `packageManager` in `package.json`, and the Node version in the workflows |
| GitHub Actions | `actions/checkout`, `actions/cache` | A full commit SHA in the workflows ([CI.md](CI.md#permissions-and-supply-chain)) |
| Services and background processes | none today | — |

## Approving a new dependency

**Who:** the Product Owner approves every new **direct** dependency, every new native library, toolchain, service or background process, and every exception to this policy. An agent or contributor proposes; the PO decides (AGENTS.md).

**When:** before the code that uses it is merged, in the issue or the PR that introduces it. A new major version of a direct dependency counts as a new approval.

**The request states:**

1. **Need:** what it's for, and why the current stack can't do it. Check first that nothing already in `Cargo.lock` or `pnpm-lock.yaml` does the job (AGENTS.md, "Reuse before you add").
2. **Licence:** the SPDX expression, and for an `OR` licence, the option V2 uses. Compare it with the allowed list below.
3. **Maintenance:** the latest release and its date, recent activity, and the maintainers.
4. **Security:** known advisories (RustSec, GitHub advisories, the vendor's own list such as curl's) and how fixes are released.
5. **Weight:** the transitive dependencies it adds (`cargo tree -p <crate>`, `pnpm why <package>`), and native build requirements.
6. **Platforms:** Windows, Linux and macOS support.
7. **For native libraries:** the exact source (official release URL), version, SHA-256, signature status and build options. A wrapper crate's version **doesn't prove** the native version it bundles: `curl-sys` 0.4.91 bundles curl 8.22.0 (research report, section 8.1).

**After approval:** add the dependency to the [register](#approved-direct-dependencies) in the same PR.

**Transitive dependencies** don't need individual approval, but they must pass the licence rules below. The notices generator (#76) lists every one and flags any that don't.

## Allowed licences

| Licence class | Licences | Rule |
|---|---|---|
| **Permissive: allowed** | MIT, MIT-0, Apache-2.0 (with or without LLVM-exception), BSD-2-Clause, BSD-3-Clause, ISC, Zlib, 0BSD, Unlicense, CC0-1.0, BSL-1.0, BlueOak-1.0.0, Unicode-3.0, the curl licence | Allowed; credited in the notices. MIT-0 and BlueOak-1.0.0 were added by the PO on #29 (2026-10-29) |
| **Fonts: allowed unmodified** | OFL-1.1 | Only for a font used as it is published. Its licence text goes in the notices. The font is never sold on its own, and a modified copy would need another name (PO decision on #29, 2026-10-29) |
| **Data for build tools: allowed** | CC-BY-4.0 | Only in a build tool's data that isn't shipped: today `caniuse-lite`, the browser data the CSS tools read (PO decision on #29, 2026-10-29) |
| **File-level copyleft: allowed unmodified** | MPL-2.0 | Allowed as a transitive dependency when used unmodified (today: Tauri's CSS stack). Modifying one needs PO approval, and the changed files must be published |
| **Copyleft: PO approval required** | LGPL-2.1/3.0, GPL-2.0/3.0, AGPL-3.0, EPL, CDDL | Never added without explicit PO approval and a licensing note in [LICENSING.md](LICENSING.md). For an `OR` licence offering a permissive option (for example `MIT OR Apache-2.0 OR LGPL-2.1-or-later`), V2 uses the permissive option |
| **Unknown or custom** | A missing licence, "see LICENSE", a proprietary EULA | Treated as not allowed until the PO approves after review |

**No aria2:** V2 never includes aria2 or its source ([ADR-0001](decisions/0001-v2-direction.md)), so its GPL obligations don't arise for V2.

## Pinning and updates

- **Lockfiles are committed and authoritative.** `Cargo.lock` and `pnpm-lock.yaml` change only in a PR that says why. Changing either selects every CI check ([CI.md](CI.md#selecting-jobs)).
- **Toolchains are pinned exactly.** A change is its own PR with a full `pnpm check:full` run.
- **GitHub Actions are pinned to full SHAs.** Dependabot proposes updates monthly, grouped into one PR ([CI.md](CI.md#permissions-and-supply-chain)).
- **Rust and npm updates aren't automated.** Each update is a PR stating the reason (a fix, an advisory, a needed feature), with the full check level. Advisory monitoring and response times are #75.
- **Native libraries** are updated only by changing the native lock file, never by fetching "latest" at build time.

## Native libraries

### Lock file

Every native library V2 builds or bundles has one line in the native lock file, [`scripts/native/deps.lock`](../scripts/native/deps.lock) (#35). The format follows V1's `scripts/aria2/deps.lock` and adds the signature status and licence. Two lines from it:

```text
# name  version  source-url  sha256  signature  licence
zlib    1.3.2   https://github.com/madler/zlib/releases/download/v1.3.2/zlib-1.3.2.tar.xz  d7a0654783a4da529d1bb793b7ad9c3318020af77667bcae35f95d0e42a792f3  verified:5ED46A6721D365587791E2AA783FCD8E58BCAFBA  Zlib
curl    8.23.0  https://curl.se/download/curl-8.23.0.tar.xz  8dadd7d2536a314f982ad05a651b713f426bf7d02cc2c76cffb9796f2218e6c5  verified:27EDEAF22F3ABCEB50DB9A125CC908FDB71E12C2  curl
```

- **One line per library,** with a comment header saying when and how the hashes were computed.
- **`sha256`** is computed from the downloaded file. The build refuses to continue if a download doesn't match.
- **`signature`** is `verified:<fingerprint>`, `unverifiable:<reason>`, or `none-published`. An unverifiable signature is recorded honestly, never assumed fine.
  - For `verified`, the fingerprint is the signer's **primary** key, and that project's published release-signing key is committed as `scripts/native/keys/<name>.asc`. The lock file's header says where each key came from.
  - The build checks the detached `<source-url>.asc` with gpg against that one key only, and refuses a signature from any other key, an expired or revoked key, or a file with more than one signature.
- **Licence** is an SPDX identifier from the permissive class in the table above. A line with any other licence is refused until the PO records an exception.
- **Enforced by** `scripts/native/native-lib.mjs` (unit tested), which `pnpm native:fetch` and `pnpm native:build` use ([BUILDING_NATIVE.md](BUILDING_NATIVE.md)).

### Libraries from vcpkg

The torrent bridge's libraries, libtorrent, Boost and OpenSSL, aren't in the lock file yet. They are built with vcpkg, pinned by the registry's baseline commit and by overlay ports with each source's SHA-512 (PO decision on #62). Their own record is `libtorrent-info.json` ([BUILDING_NATIVE.md](BUILDING_NATIVE.md#libtorrent)). Building them from verified sources like libcurl is a release blocker below.

### Build identity record

Each native build writes `target/native/<target>/build-info.json` (#35), and release packaging copies it next to its artefacts (#84). The fields are listed in [BUILDING_NATIVE.md](BUILDING_NATIVE.md#build-infojson). For every native library it records:

- the source URL, SHA-256 and signature result (the release archive identifies the upstream release);
- the configure or CMake options;
- the TLS backend and the versions of every linked library (nghttp2, zlib, libssh2…);
- the SHA-256 of every built library and of the curl tool;
- the Rust and Node toolchain versions;
- the V2 commit.

The build also runs its own curl tool (`curl -V`), records what was actually linked, and fails unless it matches the lock file: libcurl at or above the version floor (8.23.0, research report section 8.1), the pinned nghttp2, zlib and TLS backend, the required protocols and features, and no protocol V2 doesn't use.

At run time, `velocity-curl` reads what is actually linked (`curl_version_info`: version, TLS backend, protocols, features), and refuses to start libcurl when it fails the same checks ([BUILDING_NATIVE.md](BUILDING_NATIVE.md#checking-at-run-time)). `pnpm native:verify` enforces items 3 to 5 of the release blockers below, and `pnpm tauri build` runs it first.

### Distribution packages

A distribution's own dynamic libraries bring their own dependency closure. Ubuntu 24.04's libcurl 8.5.0 links **libssh 0.10.6 (LGPL-2.1), librtmp (LGPL-2.1) and libidn2 (LGPLv3+ or GPLv2+)**, as the `curl -V` line in V1's saved benchmark results shows (research report section 8.2). So:

- **Never bundle distribution libraries into a self-contained package** (an AppImage, or a Windows or macOS installer) without a licensing review and PO approval. Release builds link the libraries from the native lock file.
- Distribution packagers may link the system libcurl, but the runtime capability check (#35) still applies.

## Release blockers related to provenance

A release or release candidate isn't built while any of these is true (#17, #18, #78):

1. **A dependency without approval:** a direct dependency, native library, toolchain or action without an approval record.
2. **A licence outside the allowed list,** or an unknown licence, without a PO exception recorded in [LICENSING.md](LICENSING.md).
3. **A native library not in the lock file,** or whose download doesn't match its SHA-256.
4. **A missing or incomplete `build-info.json`.**
5. **libcurl below the version floor,** or a native library with a known unfixed advisory rated Medium or higher affecting an enabled feature (#35, #75).
6. **Out-of-date third-party notices** (#76): not regenerated for this build, or with unreviewed new entries.
7. **The torrent bridge's libraries from vcpkg** (#62): libtorrent, Boost and OpenSSL must come from V2's own build of verified sources, as libcurl does, and be checked by `pnpm native:verify`. Until then they are for development and tests only (PO decision on #62).

## Approved direct dependencies

The register of direct dependencies and how each was approved. Transitive dependencies are listed by the notices generator (#76).

| Dependency | Kind | Licence | Purpose | Approved |
|---|---|---|---|---|
| `tauri` 2, `tauri-build` 2 | Rust | Apache-2.0 OR MIT | Desktop shell | PO, with the stack in #2 (PR #92) |
| `serde` 1, `serde_json` 1 | Rust | MIT OR Apache-2.0 | Serialisation for commands and config tests | PO, #2 (PR #92) |
| `react` 19, `react-dom` 19 | npm | MIT | UI | PO, #2 (PR #92) |
| `@tauri-apps/api` 2 | npm | Apache-2.0 OR MIT | Typed calls into the shell | PO, #2 (PR #92) |
| `dirs` 7.0.0 | Rust | MIT OR Apache-2.0 | Per-OS data folder for the core and the native host (docs/COEXISTENCE.md); already in the tree through Tauri | PO, #27 (2026-10-22) |
| `tauri-plugin-single-instance` =2.5.2 | Rust | Apache-2.0 OR MIT | A second launch focuses the running window | PO, #27 (2026-10-22; 2.5.2 chosen over 2.5.4, released 2026-10-14) |
| curl 8.23.0, nghttp2 1.70.0, zlib 1.3.2; OpenSSL 3.5.9 on Linux | Native libraries, static ([BUILDING_NATIVE.md](BUILDING_NATIVE.md)) | curl, MIT, Zlib, Apache-2.0 | The direct-download engine's transport: HTTP/2, TLS (Schannel on Windows, OpenSSL on Linux), compression | PO, #35 (2026-10-22; OpenSSL's 3.5 long-term line chosen over 4.0) |
| `curl-sys` =0.4.91 (no default features) | Rust | MIT | libcurl's C API, for `crates/velocity-curl` only. Its bundled curl 8.22.0 is never built: `.cargo/config.toml` overrides its build script, and `velocity-curl` links the Velocity-built libcurl. It brings `libz-sys` (MIT OR Apache-2.0, build script overridden too) and `vcpkg` (MIT/Apache-2.0, never run) | PO, #35 (2026-10-22) |
| `curl` 0.4.51, pinned to curl-rust commit `2fb602bb1d3a` (no default features; `poll_7_68_0`) | Rust | MIT | libcurl's safe API (`Multi`, `Easy2`) for `crates/velocity-curl`. The pin carries the fix for a crash when a `Multi` is dropped with transfers attached (curl-rust #645, after 0.4.51); reviewed on #37. `curl-sys` is pinned to the same commit, identical to 0.4.91. Brings `schannel` 0.1.30 (MIT) on Windows. Switch back to crates.io once a release includes #645 (ADR-0004) | PO, #37 (2026-10-22) |
| `rusqlite` =0.40.2 (`bundled`), with `libsqlite3-sys` 0.38.2 and SQLite 3.53.2 | Rust; SQLite is compiled from the C source inside `libsqlite3-sys` | MIT (rusqlite, libsqlite3-sys); SQLite is public domain | The app database ([DATA_AND_MIGRATIONS.md](DATA_AND_MIGRATIONS.md)), for `crates/velocity-core` only. **An exception to the native lock file:** SQLite isn't downloaded by `pnpm native:fetch`; its source comes inside the crate, pinned by `Cargo.lock`'s checksum, and builds with the platform's C compiler. No advisories for these versions (GitHub advisory database: all are for 0.26 and older). sqlite.org lists CVE-2026-11822 and CVE-2026-11824 as fixed in 3.53.2 (checked 2026-10-23). It brings `hashlink`, `fallible-iterator`, `fallible-streaming-iterator`, `bitflags`, `smallvec` and `foldhash` (MIT, Apache-2.0 or Zlib) | PO, #26 (2026-10-23) |
| `rusqlite_migration` =2.6.0 (no default features) | Rust | Apache-2.0 | Numbered schema migrations on `PRAGMA user_version`, in `crates/velocity-core`; the backup before migrating and the refusal of a newer schema are V2's own code. It brings `log` (MIT OR Apache-2.0). No advisories (checked 2026-10-23) | PO, #26 (2026-10-23) |
| `windows-sys` =0.61.2 (`Win32_Foundation`, `Win32_Storage_FileSystem`, `Win32_System_IO`, `Win32_System_Ioctl`), Windows only | Rust | MIT OR Apache-2.0 | The engine's Windows file operations: sparse part files (`FSCTL_SET_SPARSE`) and the rename that never replaces (`MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`), in `crates/velocity-transfer` (#43). Maintained by Microsoft. The same version `curl` and `dirs` already brought into the build, so only its declarations are new. Test code only: `crates/velocity-test-http` adds `Win32_System_Diagnostics_Debug` to freeze a process (`DebugActiveProcess`) in the hung-engine test (#48) | PO, #43 (2026-10-23); the test use, PO, #48 (2026-10-23) |
| `libc` =0.2.191, Unix only | Rust | MIT OR Apache-2.0 | The engine's no-replace rename on Linux (`renameat2` with `RENAME_NOREPLACE`), in `crates/velocity-transfer` (#43). Maintained by the Rust project. The same version Tauri already brought into the build | PO, #43 (2026-10-23) |
| `fastrand` =2.5.0 | Rust | Apache-2.0 OR MIT | Random spread (full jitter) for retry waits, in `crates/velocity-transfer` (#48). Not a cryptographic generator, and never used as one. No dependencies of its own. No advisories in OSV (checked 2026-10-23). Maintained by smol-rs; 2.5.0, from 2026-07-19, is the latest release. The same version Tauri's build-time code generation already brought into the build | PO, #48 (2026-10-23) |
| `httpdate` =1.0.3 | Rust | MIT OR Apache-2.0 | Reads the HTTP-date form of `Retry-After` (RFC 9110 §5.6.7, all three formats), in `crates/velocity-transfer` (#48). No dependencies of its own. No advisories in OSV (checked 2026-10-23). Maintained by pyfisch; 1.0.3, from 2023-08-13, is the latest release, and the crate has been stable since | PO, #48 (2026-10-23) |
| `sha2` =0.11.1, `sha1` =0.11.0, `md-5` =0.11.0 (no default features) | Rust | MIT OR Apache-2.0 | Since #62, `sha1` and `sha2` also compute torrents' info hashes and check v2 piece layers in `crates/velocity-core` (the same crates and versions, no new code in the build). Whole-file and piece checksums in `crates/velocity-transfer` (#46): MD5, SHA-1, SHA-224, SHA-256, SHA-384 and SHA-512, the six algorithms V1 accepts. Without default features they bring only the `Digest` trait, no `alloc` or `oid`. Maintained by RustCrypto; these are the latest releases (`sha2` 0.11.1 from 2026-10-12, `sha1` 0.11.0 from 2026-03-27, `md-5` 0.11.0 from 2026-03-27). OSV lists none for `sha1` or `md-5`; `sha2`'s three (GHSA-fc7x-2cmc-8j2g, GHSA-xpww-g9jx-hp8r, RUSTSEC-2021-0100) cover only 0.9.7, fixed in 0.9.8 (checked 2026-10-23). `sha2` 0.10.9 was already in the build, but only in Tauri's build-time code generation, so these are new in the shipped app. They bring `digest` 0.11.3, `block-buffer` 0.12.1, `crypto-common` 0.2.2, `hybrid-array` 0.4.15 and `cpufeatures` 0.3.1 (all MIT OR Apache-2.0, no advisories in OSV) | PO, #46 (2026-10-23); `sha1` and `sha2` for the core's info hashes, PO, #62 (2026-10-26) |
| `cxx` =1.0.205, `cxx-build` =1.0.205 | Rust | MIT OR Apache-2.0 | The C++ bridge to libtorrent in `crates/velocity-lt` only: `cxx` checks both sides of the boundary at compile time, and `cxx-build` compiles the C++ shim. The same version V1's bridge used. Maintained by David Tolnay; 1.0.205, from 2026-10-11, is the latest release. They bring, all MIT OR Apache-2.0 (`unicode-ident` also Unicode-3.0), mostly at build time only: `cxxbridge-macro`, `cxxbridge-flags`, `cxxbridge-cmd` (with `clap` 4.6.7, `clap_builder`, `clap_lex`, `anstyle` and `strsim`, build only), `link-cplusplus`, `scratch`, `codespan-reporting`, `termcolor`, `unicode-width` and `foldhash`. No advisories in OSV for any of the 36 crates `velocity-lt` builds with (checked 2026-10-26) | PO, #62 (2026-10-26) |
| libtorrent-rasterbar 2.0.15, Boost 1.91.0, OpenSSL 3.5.9, built with vcpkg | Native libraries, static ([BUILDING_NATIVE.md](BUILDING_NATIVE.md#libtorrent)) | BSD-3-Clause (libtorrent, with parts under Zlib and in the public domain, and APSL-2.0 and BSD-4-Clause in one iOS-only header: [LICENSING.md](LICENSING.md)), BSL-1.0, Apache-2.0 | BitTorrent v1, v2 and hybrid for the torrent helper (#62). 2.0.15 (2026-09-25) is the latest 2.0 release; it has the overflow fixes of 2.0.13 and 2.0.14. libtorrent publishes no security advisories on GitHub, and OSV lists for it only Ubuntu entries for 1.1.x and older; none for Boost (checked 2026-10-26). OpenSSL 3.5.9 is the version libcurl uses. Development and tests only until release blocker 7 is resolved | PO, #62 (2026-10-26: libtorrent 2.0.15 everywhere; vcpkg now; OpenSSL 3.5.9 through an overlay port) |
| vcpkg, registry commit `9e593bb` (2026-07-29), tool 2026-07-27 | Build tool, not shipped | MIT | Builds libtorrent, Boost and OpenSSL for the torrent bridge (`pnpm native:build`). Maintained by Microsoft. It downloads its own CMake and Ninja on Linux | PO, #62 (2026-10-26) |
| `jiff` =0.2.38 (default features) | Rust | Unlicense OR MIT (V2 uses MIT) | Local time for the download schedule in `crates/velocity-core` (#50): the system time zone with its summer-time rules, so "02:00 to 07:00 on weekdays" means the user's clock. Maintained by Andrew Gallant (BurntSushi); 0.2.38, from 2026-10-06, is the latest release. No advisories in OSV (checked 2026-10-23). It brings `jiff-core` 0.1.1, and on Windows only `jiff-tzdb-platform` 0.1.3 and `jiff-tzdb` 0.1.9, a bundled copy of the IANA time-zone database, because Windows has none in that form (all Unlicense OR MIT, no advisories in OSV), plus `windows-link`, already in the build. Linux reads the system's `/usr/share/zoneinfo`. They were in `Cargo.lock` already as optional dependencies of `serde_with`, never built | PO, #50 (2026-10-23; chosen over chrono and the OS's own calls) |
| `quick-xml` =0.42.0 (no default features) | Rust | MIT | Reads Metalink documents in `crates/velocity-core` (#54) as a stream of events. It never expands custom entities, and V2 refuses any document with a `<!DOCTYPE>` ([SECURITY.md](SECURITY.md#metalink-54)). Without default features: no serde and no encodings other than UTF-8. Maintained in `tafia/quick-xml`; 0.42.0, from 2026-08-22, is the latest release. No advisories in OSV for 0.42.0 or its one dependency, `memchr` 2.8.3 (checked 2026-10-24). The same version Tauri already brought into the build through `plist`, so it adds no crate | PO, #54 (2026-10-24; chosen over roxmltree) |
| `proptest` =1.11.0 (no default features; `std`) | Rust (development only, not shipped) | MIT OR Apache-2.0 | Property tests of the range scheduler's invariants I1 to I7 over thousands of generated scenarios, with shrinking of a failing case (#42). It brings `rand` 0.9, `rand_chacha`, `rand_core`, `rand_xorshift`, `unarray`, `ppv-lite86` and `zerocopy` (all MIT OR Apache-2.0, `zerocopy` also BSD-2-Clause). No advisories in the GitHub advisory database (checked 2026-10-23); maintained by proptest-rs, last release 2026-10-10 | PO, #42 (2026-10-23; 1.11.0 chosen over 1.12.0, released 2026-10-10) |
| `h2` =0.4.20 | Rust (development only, not shipped) | MIT | The HTTP/2 fixture server in `crates/velocity-test-http` (#47), used directly rather than through hyper, which would add hyper, hyper-util and http-body-util. Maintained by the hyper project; 0.4.20, from 2026-10-06, is the latest release. No advisories in OSV for 0.4.20 (checked 2026-10-23). Its dependencies (`tokio-util`, `slab`, `indexmap`, `fnv`, `futures-core`, `futures-sink`, `atomic-waker`, `tracing`; MIT or MIT OR Apache-2.0) were already in the build through Tauri | PO, #47 (2026-10-23) |
| `tokio` =1.54.1 (`rt`, `rt-multi-thread`, `net`, `time`, `io-util`; `sync` and `macros` in `crates/velocity-sftp`) | Rust (shipped in the SFTP layer since #53, and through Tauri) | MIT | Runs the HTTP/2 fixture server (#47) and the TLS fixture (#69; `io-util` for `copy_bidirectional`, added on #69). Since #53, the SFTP layer's runtime, which russh needs. Tauri already brought tokio into the app: the lock file moved it from 1.54.0 to 1.54.1, which fixes RUSTSEC-2026-0384 (a race in `tokio::time::Interval` on the multi-threaded runtime that can corrupt memory, introduced in 1.54.0). Maintained by the tokio project; 1.54.1 was released 2026-10-21 | PO, #47 (2026-10-23; the lock file update inside #47) |
| `rustls` =0.23.46 (no default features; `ring`, `std`, `tls12`) | Rust (development only, not shipped) | Apache-2.0 OR ISC OR MIT | Terminates TLS in the TLS fixture of `crates/velocity-test-http` (#69), and in its FTPS server, synchronously on standard-library threads (#51, no new feature). No default features, so it brings no aws-lc-rs. Maintained by the rustls project; 0.23.46, from 2026-10-14, is the latest release. No advisories in OSV (checked 2026-10-24). New with it: `rustls-pki-types` 1.15.1 (MIT OR Apache-2.0), `rustls-webpki` 0.103.16 (ISC), `subtle` 2.6.1 (BSD-3-Clause), `zeroize` 1.9.1 (Apache-2.0 OR MIT); none has an OSV advisory | PO, #69 (2026-10-24) |
| `tokio-rustls` =0.26.6 (no default features; `ring`, `tls12`) | Rust (development only, not shipped) | MIT OR Apache-2.0 | rustls on tokio, for the TLS fixture (#69). Maintained by the rustls project; 0.26.6, from 2026-09-28, is the latest release. No advisories in OSV (checked 2026-10-24) | PO, #69 (2026-10-24) |
| `ring` 0.17.14 (through rustls and rcgen; through russh since #53) | Rust (shipped in the SFTP layer since #53) | Apache-2.0 AND ISC | The crypto provider of rustls and rcgen in the TLS fixture (#69), and of russh for SFTP (#53). Chosen over aws-lc-rs, which needs CMake and a C compiler on every machine (PO decision on #69). Its latest release, 0.17.14, is from 2025-03-11. No advisories in OSV (checked 2026-10-24). New with it: `untrusted` 0.9.0 (ISC), `getrandom` 0.2.17 (MIT OR Apache-2.0), and on Windows `windows-sys` 0.52.0 with its `windows-targets` 0.52.6 (MIT OR Apache-2.0); none has an OSV advisory | PO, #69 (2026-10-24) |
| `rcgen` =0.14.10 (no default features; `crypto`, `pem`, `ring`) | Rust (development only, not shipped) | MIT OR Apache-2.0 | Makes the TLS fixture's test CA, certificate variants and revocation list (#69). Maintained by the rustls project; 0.14.10, from 2026-08-28, is the latest release. No advisories in OSV (checked 2026-10-24). New with it: `pem` 4.0.0 (MIT), `yasna` 0.6.0 (MIT OR Apache-2.0); `time` was already in the build. `x509-parser` appears in the lock file but is built for no target | PO, #69 (2026-10-24) |
| `http` =1.5.0, `bytes` =1.12.1 | Rust (development only, not shipped) | MIT OR Apache-2.0 (`http`), MIT (`bytes`) | `h2`'s own request, response and buffer types, for the HTTP/2 fixture server (#47). Both are the latest releases, already in the build through Tauri. No advisories in OSV (checked 2026-10-23) | PO, #47 (2026-10-23) |
| `russh` =0.64.1 (no default features; `ring`, `rsa`) | Rust | Apache-2.0 | The SSH client of the SFTP layer, `crates/velocity-sftp` ([ADR-0013](decisions/0013-sftp-russh.md), replacing libssh2, whose latest release has 10 unfixed advisories). ring as its crypto: no aws-lc-rs, no OpenSSL. Maintained in `warp-tech/russh`; 0.64.1, from 2026-10-05, is the latest release. OSV (checked 2026-10-24) lists 23 russh advisories from 2023 to 2026, all fixed in 0.64.1. **New with it and russh-sftp: 93 crates** (191 resolved), all MIT, Apache-2.0, MIT OR Apache-2.0 or BSD-3-Clause, among them RustCrypto's ciphers, hashes and elliptic curves, `curve25519-dalek`, `ed25519-dalek` and `ml-kem`. **Three are release candidates russh pins:** `ssh-key` =0.7.0-rc.11, `rsa` =0.10.0-rc.18 and `pkcs1` =0.8.0-rc.4. **One open advisory, accepted by the PO:** RUSTSEC-2023-0071 (CVE-2023-49092, "Marvin"), a timing side channel in `rsa`'s private-key operations, with no fix; an SFTP client signs once per login with the user's own RSA key, and host-key checks use only public keys. Revisit when `rsa` ships a fix | PO, #53 (2026-10-24) |
| `russh-sftp` =3.0.1 (default features) | Rust | Apache-2.0 | The SFTP protocol on russh's channel, in `crates/velocity-sftp` (#53, ADR-0013). Maintained in `AspectUnk/russh-sftp`; 3.0.1, from 2026-09-28, is the latest release. No advisories in OSV (checked 2026-10-24) | PO, #53 (2026-10-24) |
| `zod` 4.6.6 (exact) | npm | MIT | Strict validation of every native response (docs/CONTRACTS.md). 4.6.6 was released 2026-10-11; no advisory in OSV for it (checked 2026-10-23). 4.6.7, from 2026-10-12, is the latest | PO, #25 (2026-10-22; 4.6.5 chosen over 4.6.7, released 2026-10-12); 4.6.6 as Dependabot updated it (#160), PO, #162 (2026-10-23) |
| `@tauri-apps/cli`, `vite`, `@vitejs/plugin-react`, `typescript`, `@types/*`, `eslint`, `@eslint/js`, `typescript-eslint`, `eslint-plugin-react-hooks`, `eslint-plugin-react-refresh`, `globals`, `vitest` | npm (development only) | MIT, Apache-2.0 or BSD (not shipped) | Build, types, lint, tests | PO, #2 (PR #92) |
| `@radix-ui/react-dialog` 1.3.1, `@radix-ui/react-alert-dialog` 1.2.1 (exact) | npm | MIT | Accessible dialogs: focus trap, Escape, screen-reader roles (V1's). Maintained by WorkOS; both from 2026-10-12, the latest releases. They bring 17 `@radix-ui/*` helpers and `react-remove-scroll`, `aria-hidden`, `use-sidecar` and their helpers (MIT), and `tslib` (0BSD) | PO, #29 (2026-10-29: V1's UI stack). Radix's other V1 parts come with the issues that use them |
| `lucide-react` 1.57.0 (exact) | npm | ISC | The icons, as in V1. 1.57.0 is from 2026-10-14; 1.64.0, from 2026-10-29, is the latest | PO, #29 (2026-10-29) |
| `sonner` 2.0.8 (exact) | npm | MIT | Toasts for a failed save or command (V1's). 2.0.8, from 2026-08-09, is the latest | PO, #29 (2026-10-29) |
| `zustand` 5.0.15 (exact) | npm | MIT | The window's presentation state (V1's stores). 5.0.15 is from 2026-08-13; 5.0.16, from 2026-10-22, is the latest | PO, #29 (2026-10-29) |
| `@tanstack/react-query` 5.104.2 (exact) | npm | MIT | Queries for what the native side owns: settings and categories (V1's). 5.104.2 is from 2026-10-11; 5.105.0, from 2026-10-26, is the latest. It brings `@tanstack/query-core` (MIT) | PO, #29 (2026-10-29) |
| `@fontsource-variable/inter` 5.3.0 (exact) | npm (font files, bundled) | OFL-1.1 | The Inter font, as in V1. 5.3.0, from 2026-07-19, is the latest | PO, #29 (2026-10-29: OFL-1.1 for fonts, unmodified) |
| `clsx` 2.1.1, `tailwind-merge` 3.7.0, `class-variance-authority` 0.7.1 (exact) | npm | MIT, MIT, Apache-2.0 | Joining Tailwind classes, and the button's variants (V1's). Each is the latest release (2024-04-23, 2026-09-12, 2024-11-26) | PO, #29 (2026-10-29) |
| `tailwindcss` 4.3.3, `@tailwindcss/vite` 4.3.3 (exact) | npm (development only: it writes the CSS at build time) | MIT | The styles and V1's design tokens (`src/index.css`). 4.3.3, from 2026-07-16, is the latest. They bring `lightningcss` 1.32.0 (MPL-2.0, unmodified, build time only) | PO, #29 (2026-10-29) |
| `@testing-library/react` 16.3.3, `@testing-library/dom` 10.4.2, `@testing-library/user-event` 14.6.8, `jsdom` 30.1.2 (exact) | npm (development only, not shipped) | MIT | UI tests that click and type (docs/TESTING.md). Each is the newest release at least two weeks old; jsdom 30.1.3 is from 2026-10-15. jsdom brings `@csstools/*` (MIT-0) and `lru-cache` 11 (BlueOak-1.0.0) | PO, #29 and #60 (2026-10-29) |
| `@tanstack/react-virtual` 3.14.19 (exact) | npm | MIT | The download list's virtualised rows, as V1 (#59): only the rows in view are drawn. Maintained by TanStack; 3.14.19 is from 2026-10-13, the newest release at least two weeks old. It brings `@tanstack/virtual-core` 3.19.4 (MIT). No advisories in OSV for either (checked 2026-10-29) | PO, #59 (2026-10-29) |
| `@radix-ui/react-context-menu` 2.4.1, `@radix-ui/react-dropdown-menu` 2.2.1 (exact) | npm | MIT | The list's right-click menu and its column chooser, as V1 (#59): keyboard and screen-reader menus. Maintained by WorkOS; both from 2026-10-12, the newest releases at least two weeks old. They bring `@radix-ui/react-menu`, `-popper`, `-roving-focus`, `-collection`, `-direction`, `-arrow`, `-use-rect`, `-use-size`, `-use-is-hydrated` and `rect`, and `@floating-ui/core`, `dom`, `react-dom` and `utils` (all MIT). No advisories in OSV for any of them (checked 2026-10-29) | PO, #59 (2026-10-29) |
| `@playwright/test` 1.64.1 (exact) | npm (development only, not shipped) | Apache-2.0 | The list's layout test in a real browser (L4, as V1's `tests/e2e/layout`). Maintained by Microsoft; 1.64.1 is from 2026-10-14, the newest release at least two weeks old. It brings `playwright` and `playwright-core` 1.64.1 (Apache-2.0). No advisories in OSV (checked 2026-10-29). Its browser, about 150 MB, is downloaded by `pnpm exec playwright install chromium` on the machines that run the test, never by `pnpm install` ([TESTING.md](TESTING.md)) | PO, #59 (2026-10-29) |
| `@types/chrome` 0.3.5 (exact) | npm (development only: types, not shipped) | MIT | The WebExtension API types for the browser extension (#64), as V1's extension had. From DefinitelyTyped; 0.3.5 is from 2026-10-09, the newest release at least two weeks old. It brings `@types/filesystem`, `@types/filewriter` and `@types/har-format` (MIT). The extension is built with Vite and tested with Vitest, already in V2, instead of V1's esbuild | PO, #64 (2026-10-29) |
| `tauri-plugin-dialog` =2.8.3, `tauri-plugin-notification` =2.7.0 | Rust | Apache-2.0 OR MIT | The system's folder chooser and desktop notifications, used from Rust only: the web layer is granted neither (`capabilities/default.json`, kept by a test). Maintained by the Tauri project; both from 2026-10-14 (2.8.4 and 2.8.0, from 2026-10-28, are the latest 2.x). They bring 18 crates to `Cargo.lock`: `tauri-plugin`, `tauri-plugin-fs`, `rfd` (MIT), `notify-rust`, `tauri-winrt-notification` and `mac-notification-sys` (MIT OR Apache-2.0), and `windows-sys` 0.60 with its `windows-targets` (MIT OR Apache-2.0). No advisories in OSV for any of them (checked 2026-10-29). The `tray-icon` feature of `tauri` adds no crate | PO, #29 and #60 (2026-10-29) |
| `tracing` =0.1.44 (only `std`), `tracing-subscriber` =0.3.23 (only `std` and `registry`), `regex` =1.13.1 (default features) | Rust | MIT; MIT; MIT OR Apache-2.0 | The app's log and redaction in `crates/velocity-core` and `src-tauri` (#28, [DIAGNOSTICS.md](DIAGNOSTICS.md)): `tracing` events go to V2's own in-memory layer, and `regex` finds secrets to redact, as V1's did. Maintained by the Tokio project (`tokio-rs/tracing`) and the Rust project (`rust-lang/regex`). Each is the latest release: 0.1.44 from 2025-12-18, 0.3.23 from 2026-03-13, 1.13.1 from 2026-07-15. `tracing` and `regex` were already in the build through other crates; new are `tracing-subscriber` and its `sharded-slab` 0.1.7 (MIT) and `thread_local` 1.1.10 (MIT OR Apache-2.0). No advisories in OSV for any of them (checked 2026-10-29) | PO, #28 (2026-10-29; chosen over a logger of V2's own and hand-written matching) |
| Rust 1.99.1, Node.js 24.12.0, pnpm 12.14.0 | Toolchains | MIT/Apache-2.0 (Rust), MIT | Building | PO, #2 (PR #92; pnpm chosen by the PO) |
| `actions/checkout`, `actions/setup-node`, `actions/cache`, `actions/upload-artifact`, `actions/download-artifact` | GitHub Actions (GitHub's own) | MIT | CI | PO, #12 and the CI PRs (#104, #105, #110) |
