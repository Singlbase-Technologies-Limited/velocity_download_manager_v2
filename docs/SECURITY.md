# Security

How V2 protects downloads and the people who use it. The rules every contributor follows are in [AGENTS.md](../AGENTS.md#security-and-data) and `.claude/rules/security.md`. This document records what V2 does and what was measured. It covers TLS for now; the URL and redirect policy (#67) and secret handling (#70) add their sections when they are built.

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

Users on Windows reach servers through the Windows store, not a CA file. A test adds the test CA to the machine's Root store, downloads from a valid certificate, is refused a revoked one ("… has been revoked"), and removes the CA. It needs an administrator, so it is ignored locally and run by PR checks' Windows job ([CI.md](CI.md)). Locally, as an administrator: `cargo test -p velocity-curl --test windows_store -- --ignored`. If it can't remove the CA, it says so ("REMOVE BY HAND").

### How the tests trust a test CA

- **Fresh per test:** each TLS front makes its own CA, with a unique name, and serves its own CRL on a fresh port, so a list cached by Windows never answers for another run.
- **Test-only trust:** the tests pass the CA file with velocity-curl's `test-ca` feature (`CURLOPT_CAINFO`, PO decision on #69). It replaces the platform's store for that test; verification and revocation checks stay on. An optimised build refuses to compile with the feature, so a release can't contain it, like `test-h2c`.
- **Nothing installed** on the machine, except in the Windows store test above.
