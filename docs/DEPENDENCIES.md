# Dependency policy

Every third-party component V2 builds with or ships is approved, pinned, verified, recorded and credited in its notices. This page is the policy (#9). What the licences require is assessed in [LICENSING.md](LICENSING.md).

The rule in [AGENTS.md](../AGENTS.md#engineering) is short: **a new crate, npm package, native library, service or background process needs Product Owner approval.** This page says what that approval needs, and what happens afterwards.

## What counts as a dependency

| Kind | Examples | Pinned by |
|---|---|---|
| Rust crates | `tauri`, `serde`; later `curl`, `curl-sys` | `Cargo.toml` (direct) and the committed `Cargo.lock` (everything) |
| npm packages, installed with pnpm | `react`, `@tauri-apps/api`, `vite`, `vitest` | `package.json` (direct) and the committed `pnpm-lock.yaml`; CI uses `pnpm install --frozen-lockfile` |
| Native libraries, linked or bundled | libcurl, nghttp2, zlib, a TLS library, possibly libssh2, libtorrent, Boost, OpenSSL | A native lock file (below), created with the first native library (#35) |
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
| **Permissive: allowed** | MIT, Apache-2.0 (with or without LLVM-exception), BSD-2-Clause, BSD-3-Clause, ISC, Zlib, 0BSD, Unlicense, CC0-1.0, BSL-1.0, Unicode-3.0, the curl licence | Allowed; credited in the notices |
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

Every native library V2 builds or bundles has one line in a native lock file. The file is created with the first native library, libcurl in #35, and its path is chosen there. The format follows V1's `scripts/aria2/deps.lock` and adds the signature status and licence:

```text
# name  version  source-url  sha256  signature  licence
curl    8.23.0   https://curl.se/download/curl-8.23.0.tar.xz  <sha256>  verified:<key fingerprint>  curl
nghttp2 1.68.1   https://github.com/nghttp2/nghttp2/releases/download/v1.68.1/nghttp2-1.68.1.tar.xz  <sha256>  unverifiable:<reason>  MIT
```

- **One line per library,** with a comment header saying when and how the hashes were computed.
- **`sha256`** is computed from the downloaded file. The build refuses to continue if a download doesn't match.
- **`signature`** is `verified:<fingerprint>`, `unverifiable:<reason>`, or `none-published`. An unverifiable signature is recorded honestly, never assumed fine.
- **Licence** is an SPDX identifier, as in the table above.

### Build identity record

Each release build writes `build-info.json` next to its artefacts (#35, #84). For every native library it records:

- the source SHA-256 and the upstream tag or commit;
- the configure or CMake options;
- the TLS backend and the versions of every linked library (nghttp2, zlib, libssh2…);
- the checksum of the built binary;
- the Rust and Node toolchain versions;
- the V2 commit.

At runtime the engine also reports what it actually linked (`curl_version_info`: version, TLS backend, protocols, features), and refuses to start below the version floor (libcurl 8.23.0, research report section 8.1; #35).

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
| `zod` 4.6.5 (exact) | npm | MIT | Strict validation of every native response (docs/CONTRACTS.md) | PO, #25 (2026-10-22; 4.6.5 chosen over 4.6.7, released 2026-10-12) |
| `@tauri-apps/cli`, `vite`, `@vitejs/plugin-react`, `typescript`, `@types/*`, `eslint`, `@eslint/js`, `typescript-eslint`, `eslint-plugin-react-hooks`, `eslint-plugin-react-refresh`, `globals`, `vitest` | npm (development only) | MIT, Apache-2.0 or BSD (not shipped) | Build, types, lint, tests | PO, #2 (PR #92) |
| Rust 1.99.1, Node.js 24.12.0, pnpm 12.14.0 | Toolchains | MIT/Apache-2.0 (Rust), MIT | Building | PO, #2 (PR #92; pnpm chosen by the PO) |
| `actions/checkout`, `actions/setup-node`, `actions/cache`, `actions/upload-artifact`, `actions/download-artifact` | GitHub Actions (GitHub's own) | MIT | CI | PO, #12 and the CI PRs (#104, #105, #110) |
