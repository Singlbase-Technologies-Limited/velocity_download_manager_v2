# Security

How V2 protects downloads and the people who use it. The rules every contributor follows are in [AGENTS.md](../AGENTS.md#security-and-data) and `.claude/rules/security.md`. This document records what V2 does and what was measured. It covers TLS, FTP and SFTP host keys for now; the URL and redirect policy (#67) and secret handling (#70) add their sections when they are built.

## TLS verification (#69)

The direct-download engine verifies every server certificate, with the platform's TLS stack: **Schannel** on Windows and **OpenSSL 3.5** on Linux ([BUILDING_NATIVE.md](BUILDING_NATIVE.md)). Verification is never turned off, in production or in a test, and there is no setting for it.

- **Trust:** on Windows, the Windows certificate store. On Linux, the CA bundle compiled into libcurl, `/etc/ssl/certs/ca-certificates.crt`.
- **Revocation:** Schannel checks it. libcurl with OpenSSL checks none, because V2 gives it no CRL file (`CURLOPT_CRLFILE`). See [revocation](#revocation).
- **A refused certificate** fails the download at once, as permanent, with "The server's certificate isn't trusted, so the connection was refused." and libcurl's own reason. Nothing is written: no file, no part file ([TROUBLESHOOTING.md](TROUBLESHOOTING.md#downloads)).

### What was measured

The certificate tests run real libcurl against the local TLS fixture ([TESTING.md](TESTING.md#fixture-servers-v2-needs)). Each case asserts the reason the backend gives, so a certificate refused for the wrong reason fails the test.

| Server certificate | Windows (Schannel) | Linux (OpenSSL 3.5) |
|---|---|---|
| Valid, for server authentication | Accepted | Accepted |
| No extended key usage (any purpose, RFC 5280 §4.2.1.12) | Accepted | Accepted |
| Client authentication only | Refused: `error 0x00000010` (`CERT_TRUST_IS_NOT_VALID_FOR_USAGE`) | Refused: "unsuitable certificate purpose (26)" |
| Code signing and e-mail protection only | Refused: `error 0x00000010` | Refused: "unsuitable certificate purpose (26)" |
| Untrusted issuer | Refused: "the certificate chain is incomplete" | Refused: "unable to get local issuer certificate (20)" |
| Another host (`wrong-host.invalid`) | Refused: "failed to match connection hostname (localhost) against server certificate names" | Refused: "no alternative certificate subject name matches target hostname 'localhost'" |
| Expired (2000-01-01 to 2000-01-02) | Refused: "… is not time valid" | Refused: "certificate has expired (10)" |
| Revoked, on the CRL the certificate names | Refused: "… has been revoked" | **Accepted** (no CRL check) |
| Valid, but the test CA not given | Refused: `SEC_E_UNTRUSTED_ROOT` | Refused: "unable to get local issuer certificate (20)" |

- **Purpose checks** (the CVE-2026-8367 class): both backends refuse a certificate from a trusted CA that isn't for TLS servers. The research report's open question 1 asked this of Schannel; it is answered here.
- **TLS 1.3:** both backends negotiate TLS 1.3 with the fixture, recorded on the server side.
- **HTTP/2** is negotiated through ALPN over TLS on both, and a segmented download over it completes.
- **Measured on** Windows 11 (`check:pr` locally, and PR checks' Windows job) and Linux (the WSL clean clone, and CI). **macOS: NOT RUN** (#83).

### Revocation

- **Windows:** Schannel checks every certificate's revocation through its CRL distribution point, and refuses one whose status it can't learn: "the revocation status is unknown" (found on #47). A server whose certificate names no reachable CRL or OCSP responder is therefore refused on Windows. Windows caches the lists it fetched.
- **Linux:** libcurl with OpenSSL checks revocation only with a CRL file, and V2 sets none, so a revoked certificate is accepted there.
- **This is recorded, not decided.** Whether V2 should check revocation on Linux (a CRL, OCSP, or stapling), or relax anything on Windows, is a decision for the PO; nothing here changes either.

### The Windows certificate store

Users on Windows reach servers through the Windows store, not a CA file. A test adds the test CA to the machine's Root store, downloads from a valid certificate, is refused a revoked, expired, wrong-host, untrusted-issuer and client-auth-only one, and removes the CA.

Measured on PR checks' Windows runner (run `40072038728`):

| Certificate, through the store | libcurl code | Schannel's reason |
|---|---|---|
| Valid | — | Accepted |
| Revoked | 35 | `CRYPT_E_REVOKED (0x80092010)` |
| Expired | 35 | `SEC_E_CERT_EXPIRED (0x80090328)` |
| Client authentication only | 35 | `SEC_E_CERT_WRONG_USAGE (0x80090349)` |
| Another host | 60 | `SEC_E_WRONG_PRINCIPAL (0x80090322)` |
| Untrusted issuer | 60 | `SEC_E_UNTRUSTED_ROOT (0x80090325)` |

- **A different error code on this path:** through the store, Schannel reports a refused certificate as `CURLE_SSL_CONNECT_ERROR` (35), with the reason only in its detail, for example `CRYPT_E_REVOKED (0x80092010)`. With a CA file it reports `CURLE_PEER_FAILED_VERIFICATION` (60). The engine treats both as a refused certificate: permanent, never retried. The store test found the first one classified as a transient handshake failure on CI, and the fix came in the same PR, with a regression test. It needs an administrator, so it is ignored locally and run by PR checks' Windows job ([CI.md](CI.md)). Locally, as an administrator: `cargo test -p velocity-curl --test windows_store -- --ignored`. If it can't remove the CA, it says so ("REMOVE BY HAND").

### How the tests trust a test CA

- **Fresh per test:** each TLS front makes its own CA, with a unique name, and serves its own CRL on a fresh port, so a list cached by Windows never answers for another run.
- **Test-only trust:** the tests pass the CA file with velocity-curl's `test-ca` feature (`CURLOPT_CAINFO`, PO decision on #69). It replaces the platform's store for that test; verification and revocation checks stay on. An optimised build refuses to compile with the feature, so a release can't contain it, like `test-h2c`.
- **Nothing installed** on the machine, except in the Windows store test above.
- **The FTPS server** terminates TLS itself, with the same kind of test CA, leaf and revocation list (`TestPki`), since `AUTH TLS` starts TLS part way through a connection.

## FTP and FTPS (#51)

How FTP downloads work is in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#ftp-and-ftps-51). What protects them:

- **Plain FTP is cleartext.** The login and the file travel unencrypted, so anyone on the network path can read them. The engine reports what it can encrypt per protocol (`encryption` in the capabilities), and the add form (R8) warns before a plain `ftp://` download starts (PO decision on #51).
- **Requiring encryption never falls back.** A download that requires it sends `AUTH TLS` before the login and fails at once if the server refuses, with nothing sent in clear after the greeting: no login, no file name. `ftps://` is TLS from the first byte. Both protect the data connections too (`PROT P`). Tested on Windows and Linux: a plain server, asked for encryption, never receives the login.
- **Certificates** are verified as in [TLS verification](#tls-verification-69), with the same backends, store and revocation behaviour. A refused certificate fails before the login.
- **No bounce to another host:** the data connection goes to the control connection's address; the address in a `PASV` reply is ignored. Active mode (`PORT`), where the server connects back, is never used.
- **The login stays out of storage.** It reaches the engine as `credentials`, and a login in the link is split off before the download is stored. The database test scans every file SQLite wrote for an FTP login, in the link and as credentials. The libcurl layer reads only the verb of each command it sends, so the `PASS` line isn't kept or logged.
- **An `http://` link can't require encryption:** such a spec is refused, rather than sent in clear.
- **Not verified:** servers that require the data connection to resume the control connection's TLS session ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#ftps-the-evaluation-51-asked-for)).

## Mirrors (#55)

A download's mirrors, such as a Metalink file's, are other servers than the one the user chose. How they're used is in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#mirrors-55). What protects V2:

- **No bytes are mixed unless proven the same.** A mirror serves a range only after its first answer shows the same size and encoding, and, without trusted piece hashes, the same strong validator as the download's first answer. With piece hashes, every piece is checked, and a mirror that sends other bytes is disabled after its second bad piece. Tested: a mirror with other bytes of the same size never ends up in the file.
- **No credentials to other servers.** The login and custom headers go only to the first URL's origin; a mirror on another origin gets neither (tested). A Metalink download carries none anyway (#54).
- **Every mirror is checked** like the first URL: the scheme allowlist applies to each, and one the engine may not fetch, such as `file://`, refuses the download (tested).
- **A bad mirror can't stop a download** as long as another source is left: it is cooled down or disabled, and its ranges go elsewhere.

## Metalink (#54)

A Metalink document is untrusted input, from a file the user chose or from any link. How it's read and used is in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#metalink-54). What protects V2:

- **No entity can expand.** A document with a `<!DOCTYPE>` is refused, so it can't declare an entity: no "billion laughs", and no external entity that reads a local file or a URL. Only XML's five predefined entities and character references are read. The parser is quick-xml (PO decision on #54), which never expands custom entities itself.
- **Bounded:** at most 16 MiB, 10,000 files, 256 sources per file, 32 levels of nesting and 8 KiB of text per value, read as a stream. A piece list fits the engine's limit (2 MiB of digests).
- **Safe names:** every part of a file's name must be one safe name (`naming::check_file_name`): no `..`, no absolute path, no drive letter, no `\` or `:`, no reserved Windows name, no control character, no part file's name, at most 32 subfolders deep. Two files at one path, whatever their case, or a file where another file's folder is, refuse the document. One bad name refuses the whole document, never only its file.
- **Folders inside the chosen folder:** created one level at a time; a file, a link or a reparse point in the way is refused, never followed (tested with a symbolic link on Linux and a junction on Windows).
- **No credentials from a document:** a source with a login in it is dropped, and a Metalink download sends no login, custom header or referrer. SFTP sources are dropped, since they would need a login and a host key the user confirms.
- **Off by default,** as in V1, so a link that turns out to be a Metalink file is just a file until the user turns Metalink on.

## Adding downloads (#60)

Everything the Add dialog sends is checked again natively (`Manager::add_request`); the dialog's own checks are for usability. What protects V2:

- **A login never goes in a link.** A link with one is refused (`INVALID_URL`); the login has its own fields and stays in memory, as do custom headers and a key's passphrase.
- **`.torrent` files are read only as plain files:** an absolute path, never a link or a reparse point, at most 16 MiB, parsed by V2's own bounded parser before the torrent helper sees it. Files dropped on the window reach Rust, which passes the page only plain `.torrent` files, at most 500; the manager checks each again when it is added.
- **A chosen folder is created only when the user agrees,** inside its nearest existing parent, one level at a time, never through a link.
- **Fetched `.torrent` links** go through the transfer engine with its usual TLS checks, into the app's own `torrent-links` folder, at most 16 MiB and 60 seconds. What arrives must parse as a torrent. It is kept without its trackers, whose addresses can carry a private tracker's passkey; they stay in memory, as every torrent's do (PO decision on #58). When the app starts, it deletes any plain file in that folder that no download comes from, and touches nothing else there.
- **SFTP host keys** are confirmed by the user in the window, after the download stops before logging in (below). A changed key is only explained, never offered for trust.

## SFTP host keys (#53)

SSH has no certificate authority to vouch for a server, so a host key is trusted only once the user has seen and confirmed its fingerprint, and it is pinned from then on (research report section 8). How it works is in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#host-keys); the backend is russh ([ADR-0013](decisions/0013-sftp-russh.md)).

- **Nothing before the key is checked:** the host key is checked against V2's own known_hosts file before the login, so no password, key signature or file name goes to an unverified server.
- **Unknown keys wait for the user.** The download fails with the key's SHA-256 fingerprint, and only the key the engine actually saw with that fingerprint can be trusted.
- **Changed keys are always refused,** and are never offered for trust. A pinned key is never replaced by trusting another.
- **V2's own file,** `<data folder>/known_hosts`, unhashed and readable. `~/.ssh/known_hosts` is neither read nor written.
- **Host certificates are refused:** V2 trusts no SSH certificate authority.
- **Logins:** a password, or a private key file with an optional passphrase. Both travel like other credentials: in memory, and through the engine's pipe, never its arguments, environment, database, checkpoints or logs ([ENGINE_CONTRACT.md](ENGINE_CONTRACT.md)). `Debug` never prints a passphrase (tested). The keychain is #70's.
- **Tested** against the installed OpenSSH server on Windows and Linux, as an unprivileged `sshd` with throwaway host keys (Ed25519, ECDSA and RSA): an unknown key refused until trusted, a changed key refused, key logins with and without a passphrase, a refused login. Password logins are tested on CI's Linux runner, against an `sshd` run as root; on Windows they are NOT RUN.
- **An accepted advisory:** `rsa` (used for RSA keys) has RUSTSEC-2023-0071 ("Marvin"), a timing side channel in private-key operations, with no fix. The PO accepted it on #53: an SFTP client signs once per login with the user's own RSA key, and host-key checks use only public keys ([DEPENDENCIES.md](DEPENDENCIES.md)).
