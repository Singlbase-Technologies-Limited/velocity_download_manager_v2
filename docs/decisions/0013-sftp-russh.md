# ADR-0013: SFTP uses russh, in its own crate behind a shared transport interface

- **Status:** Accepted
- **Date:** 2026-10-24
- **Decided by:** the Product Owner
- **Issue:** #53 (supersedes [ADR-0011](0011-sftp-libssh2.md), #52)

## Context

[ADR-0011](0011-sftp-libssh2.md) chose libssh2 through libcurl for SFTP. It also named its own revisit condition: "if libssh2 has an unfixed advisory affecting SFTP".

Before adding libssh2 to the native build, #53 checked it against OSV (`api.osv.dev`, the Debian and Ubuntu records) on 2026-10-24.

- **libssh2's latest release,** 1.11.1 (2024-10-16, signed by Daniel Stenberg), has 10 open advisories published between May and July 2026.
- **Several can be triggered by a malicious server,** some before the login: CVE-2026-66035 (a heap buffer overflow), CVE-2026-55200 (an out-of-bounds write), CVE-2026-66033 (an integer underflow with AES-GCM) and CVE-2026-66032 (a double free when an SFTP file is opened).
- **The fixes are only commits** on libssh2's main branch, from 2025-10 to 2026-07. No release contains them; the newest tag is still `libssh2-1.11.1`.

The full list is on #53. The options put to the PO were:
- build a pinned main-branch commit, with releases blocked;
- pause #53 and do #54 first;
- build 1.11.1 with V2's own patch files;
- russh.

## Decision

1. **SFTP uses russh 0.64.1 and russh-sftp 3.0.1,** with ring as their crypto: no aws-lc-rs and no OpenSSL. The PO approved both, as shipped dependencies, with RSA support.
2. **libssh2 is not used,** and the native build is unchanged. The decision on #53 to build OpenSSL on Windows for libssh2 lapses.
3. **A crate of its own:** `crates/velocity-sftp` is the only crate that calls russh, just as `velocity-curl` is the only one that calls libcurl.
4. **One transport interface:** the engine fetches through `velocity_curl::Transport`. libcurl's loop and the SFTP client both implement it, and both answer a fetch with the same events: a head, the body, then how it ended. So the probe, gate, checkpoints, retries and completion gate are shared, not duplicated. The SFTP client writes through a `FetchPipe`, which has the same bounded buffer, speed limits and body limit as libcurl's fetches.
5. **The SFTP security rules of ADR-0011 stand:**
   - an unknown host key is refused until the user confirms its fingerprint;
   - a changed key is always refused;
   - V2's own known_hosts file is unhashed, with a SHA-256 pin per host.

   All three are proven against a local OpenSSH server (PO decision on #53). The libcurl version floor of ADR-0011 no longer concerns SFTP.

## Options considered

| Option | For | Against |
|---|---|---|
| A. A pinned libssh2 main-branch commit, releases blocked until a fixed release | Keeps ADR-0011's single C stack; #53 could go ahead | Ships unreleased C code with no signature; no date for a fixed release |
| B. Pause #53, do #54 first | No unreleased code in the build | SFTP has no date |
| **C. russh (chosen)** | Memory-safe Rust, released fixes for every russh advisory OSV lists (all fixed in 0.64.1); no native build change | A second transport stack (made one interface by decision 4); 93 crates new to V2; three release candidates pinned by russh; one open advisory in `rsa` (below) |
| D. libssh2 1.11.1 plus V2's own patch files | Keeps a signed base | V2 maintains security backports itself, and could miss one |

## Consequences

- **SFTP shares the engine.** It gets the same probe, checkpoints, retries, speed limits and completion gate as HTTP and FTP, through the transport interface. Like FTP, it uses one connection and one stream until #77 measures whether more help.
- **Dependencies:** 93 crates new to V2, all MIT, Apache-2.0 or BSD-3-Clause, recorded in [DEPENDENCIES.md](../DEPENDENCIES.md).
  - **Release candidates russh pins:** `ssh-key 0.7.0-rc.11`, `rsa 0.10.0-rc.18` and `pkcs1 0.8.0-rc.4`.
  - **One open advisory, accepted by the PO:** RUSTSEC-2023-0071 ("Marvin", CVE-2023-49092), a timing side channel in `rsa`'s private-key operations, with no fix. An SFTP client signs once per login with the user's own RSA key, and host-key checks use only public keys. To be revisited when `rsa` ships a fix.
- **Version:** an approval covers the exact versions above. An update is its own reviewed change, with OSV checked again.
- **The engine contract:**
  - grows `TransferSpec::ssh_key`, `TransferError::host_key` and `DirectTransfer::trust_host_key` (`TRANSFER_CONTRACT_VERSION` 10);
  - the UI's error type grows `hostKey` (`COMMAND_API_VERSION` 6).
- **Documents:**
  - [ARCHITECTURE.md](../ARCHITECTURE.md) names the new crate;
  - [ENGINE_INTERNALS.md](../ENGINE_INTERNALS.md#sftp-53) describes SFTP;
  - [SECURITY.md](../SECURITY.md#sftp-host-keys-53) describes the host-key rules.

## Follow-up

- **#58:** the app's command that calls `Manager::trust_host_key`, and retrying the download afterwards.
- **#60:** the dialog that shows the fingerprint.
- **#70 and #71:** passphrases and passwords in the OS keychain, and the secret-leak scan.
- **#77:** whether more SFTP connections help.
- **Revisit** if russh or russh-sftp has an unfixed advisory affecting the client, or if `rsa` ships a fix for RUSTSEC-2023-0071.
