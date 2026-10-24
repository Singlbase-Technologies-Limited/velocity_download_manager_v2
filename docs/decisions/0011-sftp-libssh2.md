# ADR-0011: SFTP uses libssh2 through libcurl

- **Status:** Superseded by ADR-0013
- **Date:** 2026-10-22
- **Decided by:** the Product Owner
- **Issue:** #52

## Context

SFTP is a required protocol for V2 ([ADR-0001](0001-v2-direction.md)). The research report compared two backends (section 2, "SFTP backends"):

- **libssh2 through libcurl:** BSD-3-Clause, C. libcurl speaks SFTP through it, so SFTP shares the transfer stack with HTTP and FTP. `curl-sys`'s bundled build has no libssh2, but V2 builds libcurl itself ([ADR-0004](0004-libcurl-build-path.md), #35), so adding libssh2 is a build option.
- **russh with russh-sftp:** Apache-2.0, pure Rust and Cargo-only. It's a second protocol stack, with a younger security record and a crypto dependency closure still to list.

#52 planned a test of both backends against the same checks before choosing. The PO decided without it, preferring one transfer stack.

## Decision

1. **SFTP uses libssh2 through libcurl.** libssh2 joins the native libraries V2 builds and pins in `scripts/native/deps.lock`: version, source, SHA-256, signature and licence, as for curl (docs/BUILDING_NATIVE.md).
2. **russh is not used.**
3. **The SFTP security rules still apply in full,** and #53 must prove them against a local OpenSSH fixture before SFTP ships:
   - an unknown host key is refused until the user confirms its fingerprint;
   - a changed key is always refused;
   - the known_hosts file is V2-owned and unhashed, with a SHA-256 pin per host;
   - libcurl must be at least 8.23.0, for the CVE-2026-87865 fix.

   (Research report section 8; `.claude/rules/security.md`.)

## Options considered

| Option | For | Against |
|---|---|---|
| **A. libssh2 through libcurl (chosen)** | One transfer stack: SFTP gets the engine's scheduler, retries, limits and resume like HTTP and FTP. Fits the native build V2 already has. BSD-3-Clause | More C code. The libssh2 release and its advisories must be checked at build time. The test of both backends was skipped |
| B. russh with russh-sftp | Memory-safe Rust; no native build needed | A second transfer path just for SFTP; a younger security record; crypto dependencies still to list |
| C. Run both against the same checks first (the plan in #52) | Evidence before choosing | Time spent prototyping a backend that may be thrown away |

## Consequences

- **#53** (SFTP downloads) is unblocked once the engine exists. It adds libssh2 to the native build and the lock file, with its version, source, signature and licence checked when added, and enables `sftp` in the libcurl build's allowed protocols (`ALLOWED_PROTOCOLS` already lists it).
- **Approval:** this decision approves libssh2 as the SFTP library. The exact release and its source are recorded in the PR that adds it (docs/DEPENDENCIES.md).
- **The skipped test leaves a gap:** if #53's host-key, resume or security checks can't be met with libssh2, the PO revisits this ADR.

## Follow-up

- **#53:** SFTP downloads, with strict host-key verification, password and key authentication, and resume.
- **Revisit** if libssh2 has an unfixed advisory affecting SFTP, or fails #53's security checks.
