# Licensing assessment

**This is an engineering assessment, not legal advice.** Have it reviewed by someone qualified before distributing any build. The rules for adding dependencies are in [DEPENDENCIES.md](DEPENDENCIES.md).

## Status

- **V2's own licence hasn't been chosen** (#86, PO only; no agent selects it). There is no `LICENSE` file: all rights are reserved, `package.json` says `UNLICENSED`, and the Rust workspace sets `publish = false`. Our two crates declare no licence yet.
- **Nothing has been distributed.** Before any build goes to anyone outside the project, three things must be in place: the product licence, the third-party notices ([below](#third-party-notices), #76) and the release checks (#78).
- **V2 contains no aria2** ([ADR-0001](decisions/0001-v2-direction.md)), so V1's GPL obligations for aria2 don't apply to V2.

## Third-party components today

Measured on 2026-10-22 at the development state of `main`:

- **Rust:** `cargo tree -e normal --target all -p velocity-desktop-v2 --prefix none -f '{p}|{l}'` lists **356 third-party crates**, mostly Tauri's.
- **npm:** `pnpm licenses list --prod` lists **4 production packages**: `react`, `react-dom` and `scheduler` (MIT), and `@tauri-apps/api` (Apache-2.0 OR MIT).

Most crates are MIT, Apache-2.0, BSD, ISC or Zlib, often as `OR` choices. These need attention:

| Component | Licence | Obligation |
|---|---|---|
| `cssparser`, `cssparser-macros`, `selectors`, `dtoa-short`, `option-ext` (5 crates, pulled in by Tauri and WRY) | MPL-2.0 | File-level copyleft. Unmodified use needs the notice and a pointer to the source. Modifying them would require publishing the changed files, so it needs PO approval ([DEPENDENCIES.md](DEPENDENCIES.md#allowed-licences)) |
| `icu_*`, `zerovec`, `yoke`, `zerofrom`, `tinystr`, `litemap`, `writeable`, `potential_utf`, `zerotrie`, and `unicode-ident` in part (19 crates) | Unicode-3.0 | Permissive; keep the licence text in the notices |
| `r-efi` (2 versions) | MIT OR Apache-2.0 OR LGPL-2.1-or-later | V2 uses it under MIT or Apache-2.0 |

| `@fontsource-variable/inter` (the Inter font, since #29) | OFL-1.1 | The font files are bundled unmodified. Ship its licence text in the notices; never sell the font on its own (PO decision on #29) |

No GPL, AGPL or LGPL-only component is in the shipped graph.

**Since #29 (2026-10-29):**
- `pnpm licenses list --prod` lists **40 production packages**: React and `@tauri-apps/api` as before, plus Radix's dialogs and their helpers, TanStack Query, zustand, sonner, lucide-react (ISC), clsx, tailwind-merge, class-variance-authority (Apache-2.0), `tslib` (0BSD) and the Inter font (OFL-1.1).
- The dialog and notification plugins add 18 crates to `Cargo.lock`, all MIT or Apache-2.0 ([DEPENDENCIES.md](DEPENDENCIES.md#approved-direct-dependencies)).
- Development-only packages under MIT-0, BlueOak-1.0.0 and CC-BY-4.0 are allowed as [DEPENDENCIES.md](DEPENDENCIES.md#allowed-licences) says. None is shipped.

## Planned native libraries

From the research report, section 8.2. Versions are proposals until #35 records them in the native lock file (the build path is [ADR-0004](decisions/0004-libcurl-build-path.md)); "static" means linked into V2's own binaries.

| Component | Licence | Obligation, as distributed statically |
|---|---|---|
| libcurl 8.23.0 or later | curl (MIT-like) | Keep the copyright notice |
| `curl` and `curl-sys` crates | MIT | Notice |
| nghttp2 | MIT | Notice |
| zlib | Zlib | Notice (good practice) |
| TLS: Schannel (Windows) | Operating-system component, not redistributed | None |
| TLS: OpenSSL 3.x (Linux, macOS; on Windows, in the torrent helper only, #62) | Apache-2.0 | Licence text and the NOTICE file |
| libtorrent-rasterbar 2.0.15 (#62) | BSD-3-Clause, with parts under other licences (below) | Notice |
| Boost 1.91 (libtorrent's dependency) | BSL-1.0 | Notice |
| `cxx` crate (the torrent bridge) | MIT OR Apache-2.0 | Notice |

**libtorrent's other licences** (its `LICENSE` file at v2.0.15, checked 2026-10-26; vcpkg's recipe lists them too):

- **Zlib, and a Zlib-style licence:** `src/puff.cpp`, which libtorrent uses to read gzip-compressed tracker replies, and its ed25519 code (Orson Peters). Both are built on every platform. Notice.
- **Public domain:** `src/sha1.cpp` and `src/sha256.cpp`. Built; no obligation.
- **BSL-1.0:** the Python bindings (`bindings/python/src/`), which V2 doesn't build.
- **APSL-2.0 and BSD-4-Clause:** one header, `include/libtorrent/aux_/route.h`, a copy of the BSD routing-table header. `src/enum_net.cpp` includes it only for iPhone builds (`TORRENT_USE_SYSCTL` with `TARGET_OS_IPHONE`); macOS and the BSDs use the system's `net/route.h`, and Windows and Linux use neither. So it isn't in V2's Windows or Linux builds. BSD-4-Clause has an advertising clause, so building V2 for iOS would need a licensing review first.

**SFTP** is russh, a Rust crate (Apache-2.0), not a native library: [ADR-0013](decisions/0013-sftp-russh.md) replaced libssh2 ([ADR-0011](decisions/0011-sftp-libssh2.md)) before it shipped. It is listed with the Rust crates in the notices.

**Conclusion (research report section 8.2):** a static libcurl with Schannel (Windows) or a static OpenSSL (Linux and macOS), plus nghttp2 and zlib, contains **no copyleft component**, with either SFTP option. That **doesn't hold for a distribution's dynamic libcurl**. On Ubuntu 24.04 it brings libssh (LGPL-2.1), librtmp (LGPL-2.1) and libidn2 (LGPLv3+ or GPLv2+), whose relinking and source obligations would apply if a package bundled them, for example in an AppImage. So distribution libraries are never bundled without review ([DEPENDENCIES.md](DEPENDENCIES.md#distribution-packages)).

## Third-party notices

[`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md) lists every third-party component V2 ships, with its version, its licence, the licence V2 uses it under and its source, followed by the licence texts (#76). It goes with every distributed build.

- **Regenerate it** with `pnpm notices` after any dependency change, and commit the result. `pnpm check:notices` fails when the file is out of date, and when a component's licence isn't in [DEPENDENCIES.md](DEPENDENCIES.md#allowed-licences)'s allowed list. CI runs it ([CI.md](CI.md#permissions-and-supply-chain)).
- **What it reads** (`scripts/notices.mjs`):
  - **Rust:** `cargo tree -e normal --target all` for the four programs V2 ships: the app, the transfer engine, the torrent helper and the native host. So it lists crates for every target platform, including macOS-only ones, and leaves out build and test tools. `cargo metadata` gives each crate's licence, source and files.
  - **npm:** `pnpm licenses list --prod`, the packages in the user interface.
  - **Native libraries:** [`scripts/native/deps.lock`](../scripts/native/deps.lock), the vcpkg overlay ports' versions (libtorrent, OpenSSL), and Boost, whose version comes with the vcpkg baseline (a test fails when the baseline changes, so the version is checked again).
- **Which licence:** for a choice such as `MIT OR Apache-2.0`, V2 uses the first allowed licence in this order: MIT, Apache-2.0, BSD-3-Clause, BSD-2-Clause, ISC, Zlib, BSL-1.0, Unicode-3.0, 0BSD, then the rest of the permissive list. So `r-efi`'s `MIT OR Apache-2.0 OR LGPL-2.1-or-later` is used under MIT. Every part of an `AND` must be allowed.
- **Licence texts** (PO decision on #76, 2026-10-30): each distinct text is printed once, naming every component that ships it. They come from each package's own licence, copyright and notice files, only for the licences it is used under: a crate used under MIT contributes `LICENSE-MIT`, not `LICENSE-APACHE`. Native libraries' texts are in `scripts/native/licenses/`, copied from each verified source archive or from vcpkg's installed copy of it. 43 crates and one npm package ship no licence file. For those, the generator uses SPDX's standard text of their licence, from [SPDX license-list-data](https://github.com/spdx/license-list-data) v3.29.0 in `scripts/notices/licenses/`, and names the authors in their package metadata.
- **Exceptions:** a component the rules refuse needs the PO's approval and a note here; it is then listed in `APPROVED_EXCEPTIONS` in `scripts/notices-lib.mjs`. There are none.
- **Not covered yet:** per-platform closure reports attached to release-candidate builds moved to #17, and the AppImage's content inventory to #82 (PO decision on #76).

## Choosing V2's licence

Decided by the PO in #86. The inputs from this assessment:

- **Third-party components:** with the components above, the choice of V2's own licence is unconstrained. They're permissive or file-level copyleft (MPL-2.0, unmodified), and none forces a licence on V2's own code.
- **Once chosen,** set it in `LICENSE`, in `license` in `package.json` and in the workspace `Cargo.toml`, and in the README and the app's About text.
