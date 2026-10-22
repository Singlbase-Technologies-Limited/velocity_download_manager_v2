# ADR-0008: V2 has full proxy settings in its first release

- **Status:** Accepted
- **Date:** 2026-10-22
- **Decided by:** the Product Owner
- **Issue:** #56

## Context

Some networks, such as companies and schools, only allow internet access through a proxy. V1 had no proxy settings (no proxy fields in V1's `models.rs`). libcurl supports HTTP, HTTPS and SOCKS proxies with authentication. curl 8.22.0 and earlier could leak host credentials to an HTTP proxy (CVE-2026-86414, fixed in 8.23.0; research report section 8.1). The research recommended adding proxy settings after parity; the PO chose the first release.

## Decision

**Full proxy settings in the first release:**

- **Proxy types:** HTTP, HTTPS and SOCKS, with SOCKS5 required, and SOCKS4 if libcurl handles it cleanly. Each with an optional username and password.
- **Secrets:** proxy passwords are kept in the **operating system's keychain** (Windows Credential Manager, macOS Keychain, Linux Secret Service), never in the database, settings file, logs, checkpoints or process arguments.
- **No ambient proxies:** V2 always ignores proxy environment variables (`http_proxy`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`). The proxy is set explicitly from V2's settings, and "no proxy" is set explicitly too.
- **Version floor:** libcurl 8.23.0 or later ([ADR-0004](0004-libcurl-build-path.md)), for the CVE-2026-86414 fix.

## Options considered

| Option | For | Against |
|---|---|---|
| **Full proxy settings now (chosen)** | Users behind a proxy can use V2 from the start | More work and testing before the first release; a keychain dependency |
| None in the first release, add later (research recommendation) | Less up-front work; same as V1 | Users behind a proxy can't use V2 until later |
| Follow the system proxy only | No settings screen | Platform differences; PAC files; less control |

## Consequences

- **Settings and UI** (#30) gain a proxy section: type, host, port, optional username, and a password stored through the keychain.
- **Engine** (#40): every transfer sets the proxy explicitly. Credentials are scoped so they're never sent to the wrong host.
- **Secrets** (#70): a **keychain library is a new dependency that needs PO approval** ([DEPENDENCIES.md](../DEPENDENCIES.md)) before #70 adds it. Without a keychain, a password is held in memory only, for the session.
- **Tests:** local HTTP and SOCKS5 proxy fixtures, checking that credentials are never logged ([TESTING.md](../TESTING.md)).
- **Parity map:** this is a **V2 addition**, not a V1 feature. It's listed below the parity tables, not as a parity row.
