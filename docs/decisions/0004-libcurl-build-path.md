# ADR-0004: A reviewed git pin of curl-sys for the prototype; V2 builds libcurl itself for releases

- **Status:** Accepted
- **Date:** 2026-10-22
- **Decided by:** the Product Owner
- **Issue:** #34

## Context

V2's transfer engine uses libcurl ([ADR-0001](0001-v2-direction.md)) through the Rust `curl` and `curl-sys` crates, behind `crates/velocity-curl` ([ADR-0002](0002-engine-crate-layout.md)). The research report sets a version floor of **libcurl 8.23.0** (section 8.1). The latest `curl-sys` doesn't meet it. These facts were re-checked at decision time:

| Fact (2026-10-22) | Source |
|---|---|
| `curl-sys` 0.4.91+curl-8.22.0 and `curl` 0.4.51 are the latest releases | crates.io API |
| curl 8.22.0 is the last affected version for 21 advisories fixed in 8.23.0, including CVE-2026-92392 (High) and CVE-2026-87865 (libssh2 known hosts) | [curl 8.22.0 vulnerability table](https://curl.se/docs/vuln-8.22.0.html) |
| The update to curl 8.23.0 (curl-rust PR #692) is **open** | GitHub API |
| The bundled Linux build uses `select()`, so descriptors ≥ 1024 fail (issue #684, fix PR #685) **open** | GitHub API |
| The drop-order segfault fix (PR #645) was merged 2026-10-10, after `curl` 0.4.51 | GitHub API |
| `curl-sys`'s bundled build has no libssh2 (so no SFTP) and no HTTP/3 | `curl-sys` `build.rs` |

## Decision

1. **Prototype (#37): option C.** A `[patch.crates-io]` git pin of `curl` and `curl-sys` to one **reviewed** curl-rust commit that bundles curl 8.23.0 or later and has the `poll()` fix.
   - The commit, its diff review and the bundled curl version (checked at runtime with `curl_version_info`) are recorded in the PR that adds the pin.
   - The pin is for the prototype only; no release is built with it.
2. **Releases: option A.** V2 builds libcurl itself from the official `curl-<version>.tar.xz`, verified by SHA-256 and the release signature, and `curl-sys` links that build.
   - It's recorded in the native lock file and `build-info.json` ([DEPENDENCIES.md](../DEPENDENCIES.md#native-libraries)).
   - The build options decide the TLS backend (Schannel on Windows, static OpenSSL on Linux and macOS), nghttp2, zlib and, if #52 chooses it, libssh2.

## Options considered

| Option | For | Against |
|---|---|---|
| **A. V2 builds libcurl itself** (chosen for releases) | A provable identity; independent of upstream release timing; allows SFTP through libssh2 and, later, HTTP/3 | Build work on every platform (Windows: MSVC with CMake or vcpkg) |
| B. Wait for an upgraded `curl-sys` release | Simplest | No date for the upgrade; still no SFTP; the prototype waits |
| **C. A git pin to a reviewed curl-rust commit** (chosen for the prototype) | Fast; gets 8.23.0 and the fixes now | Not a release; a git dependency must be reviewed and replaced |

## Consequences

- **#37 (prototype):** may add `curl` and `curl-sys` through the pin. The dependency approval (DEPENDENCIES.md) applies: the PR states the licence (MIT) and the reviewed commit.
- **#35 (provenance):** builds the release path: the native lock file, `build-info.json`, the version floor and the runtime capability check. Releases are blocked until it exists (DEPENDENCIES.md, "Release blockers").
- **#53 and #52 (SFTP):** the libssh2 option depends on the release build path (A). The bundled crate can't provide it.
- **Revisit:** if an upgraded `curl-sys` release with 8.23.0 or later and the `poll()` fix appears before #35, the prototype pin can switch to it. The release path stays A unless a new ADR changes it.
