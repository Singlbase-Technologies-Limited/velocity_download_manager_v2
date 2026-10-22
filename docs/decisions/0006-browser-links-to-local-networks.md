# ADR-0006: Browser links to loopback or private addresses need the user's confirmation

- **Status:** Accepted
- **Date:** 2026-10-22
- **Decided by:** the Product Owner
- **Issue:** #68

## Context

A web page can make the browser extension hand V2 a link whose host resolves to the user's own machine (`127.0.0.1`, `::1`) or to a private network: a router's admin page, a NAS, an internal service. Fetching such a link on a page's behalf is a server-side request forgery risk, and DNS rebinding makes a harmless-looking name resolve to a private address at connect time. V1 accepted browser links after validation and rate limiting, with no check of the connected address (V1 `docs/BROWSER_EXTENSION.md`; parity map). libcurl can inspect the connected address before the request (`CURLOPT_PREREQFUNCTION`; research report section 8).

## Decision

**Warn and require confirmation.**

- **What it covers:** links that arrive from the browser extension (native messaging) and resolve to a loopback, link-local or private address, IPv4 or IPv6, checked on the address actually connected to (so DNS rebinding is covered).
- **What V2 does:** shows the address the link really points to and downloads only after the user confirms.
- **Typed or pasted links** are unaffected.

## Options considered

| Option | For | Against |
|---|---|---|
| **Warn and confirm (chosen)** | Stops silent requests to local services; genuine local downloads still work with one click | One extra step for legitimate local downloads started from the browser |
| Block | Strongest protection | Breaks legitimate downloads from local servers started from the browser |
| Allow (V1 behaviour) | No friction | Leaves the request-forgery risk open |

## Consequences

- **Where it's built:** #67 checks the connected address in the engine (the pre-request callback) and classifies it. Verify the libcurl callback against the docs for the version in use.
- **User interface:** the browser bridge (#65) and the Add flow (#60) carry the request's origin, so the warning applies to browser requests only.
- **Tests:** a fixture covers a browser request to `127.0.0.1`, to a private address, and to a name that resolves to a private address, plus a typed link that isn't affected (docs/TESTING.md, redirect and proxy fixtures).
