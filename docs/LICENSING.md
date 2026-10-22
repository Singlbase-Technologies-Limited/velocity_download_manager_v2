# Licensing assessment

**This is an engineering assessment, not legal advice.** Have it reviewed by someone qualified before distributing any build. The rules for adding dependencies are in [DEPENDENCIES.md](DEPENDENCIES.md).

## Status

- **V2's own licence hasn't been chosen** (#86, PO only; no agent selects it). There is no `LICENSE` file: all rights are reserved, `package.json` says `UNLICENSED`, and the Rust workspace sets `publish = false`. Our two crates declare no licence yet.
- **Nothing has been distributed.** Before any build goes to anyone outside the project, three things must be in place: the product licence, the third-party notices (#76) and the release checks (#78).
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

No GPL, AGPL or LGPL-only component is in the shipped graph.

## Planned native libraries

From the research report, section 8.2. Versions are proposals until #35 records them in the native lock file (the build path is [ADR-0004](decisions/0004-libcurl-build-path.md)); "static" means linked into V2's own binaries.

| Component | Licence | Obligation, as distributed statically |
|---|---|---|
| libcurl 8.23.0 or later | curl (MIT-like) | Keep the copyright notice |
| `curl` and `curl-sys` crates | MIT | Notice |
| nghttp2 | MIT | Notice |
| zlib | Zlib | Notice (good practice) |
| TLS: Schannel (Windows) | Operating-system component, not redistributed | None |
| TLS: OpenSSL 3.x (Linux, macOS; optional on Windows) | Apache-2.0 | Licence text and the NOTICE file |
| SFTP: libssh2 (chosen, [ADR-0011](decisions/0011-sftp-libssh2.md); added with #53) | BSD-3-Clause | Notice |
| libtorrent-rasterbar 2.0.x (#62) | BSD-3-Clause | Notice |
| Boost (libtorrent's dependency) | BSL-1.0 | Notice |

**Conclusion (research report section 8.2):** a static libcurl with Schannel (Windows) or a static OpenSSL (Linux and macOS), plus nghttp2 and zlib, contains **no copyleft component**, with either SFTP option. That **doesn't hold for a distribution's dynamic libcurl**. On Ubuntu 24.04 it brings libssh (LGPL-2.1), librtmp (LGPL-2.1) and libidn2 (LGPLv3+ or GPLv2+), whose relinking and source obligations would apply if a package bundled them, for example in an AppImage. So distribution libraries are never bundled without review ([DEPENDENCIES.md](DEPENDENCIES.md#distribution-packages)).

## Third-party notices

The notices generator is built in #76. This is the plan it follows:

- **Inputs:**
  - `cargo tree -e normal --target all` for the shipped binaries;
  - `pnpm licenses list --prod` (or `pnpm list --prod --json` with each package's `package.json`);
  - the native lock file and `build-info.json`.
- **Output:** `THIRD_PARTY_NOTICES.md`, listing each component's name, version, licence, source and the required licence texts. It's bundled with every distributed build.
- **Checks:** it flags any licence outside the allowed list and any unknown licence. Regenerating it is a release blocker ([DEPENDENCIES.md](DEPENDENCIES.md#release-blockers-related-to-provenance)).

## Choosing V2's licence

Decided by the PO in #86. The inputs from this assessment:

- **Third-party components:** with the components above, the choice of V2's own licence is unconstrained. They're permissive or file-level copyleft (MPL-2.0, unmodified), and none forces a licence on V2's own code.
- **Once chosen,** set it in `LICENSE`, in `license` in `package.json` and in the workspace `Cargo.toml`, and in the README and the app's About text.
