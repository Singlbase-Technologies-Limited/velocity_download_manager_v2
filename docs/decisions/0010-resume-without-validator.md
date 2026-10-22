# ADR-0010: Without a validator, resume after a length check and show the download as "not verified"

- **Status:** Accepted
- **Date:** 2026-10-22
- **Decided by:** the Product Owner
- **Issue:** #44

## Context

On resume, V2 must know whether the file on the server is still the one it started downloading. Bytes from two versions must never be mixed (research report section 5.3; `.claude/rules/transfer-engine.md`, "Validators").

A strong ETag or a strong `Last-Modified` proves it, and the engine sends it in `If-Range` (#45). Some servers send neither. The only check left is then the total length, which misses a file that changed without changing size.

V1 resumes in that case. The research recommended the same, with a visible flag (report decision 8).

## Decision

When a server gives no strong validator:

1. **V2 may resume after a pause or a crash**, if the server still reports the same total length as when the download started.
2. **A changed length means a changed object:** the old bytes are discarded and the download restarts once (#45).
3. **The download is marked "not verified"** in the UI and in its record, and is never shown as "Verified" (#46).
4. **A user-supplied or trusted checksum** replaces the flag with a real check, once the whole file has been hashed (#46).

## Options considered

| Option | For | Against |
|---|---|---|
| **A. Resume after a length check, with a visible "not verified" flag (chosen)** | Same behaviour as V1. A large download keeps its progress on such servers | A file changed to the same length would mix two versions without anyone noticing. The flag tells the user it wasn't proven |
| B. Restart from zero unless a checksum is supplied | Never mixes versions | Large downloads on such servers lose all progress after every pause or crash |
| C. Resume only with a user checksum | Every resumed file is proven | Most users don't have a checksum, so in practice this behaves like B |

## Consequences

- **#45** (changed-object detection) is unblocked: it implements rules 1 and 2.
- **#46** (integrity) implements rules 3 and 4: "not verified" stays until a checksum or trusted hash is checked.
- **#43** (checkpoints) stores the length and the absence of a validator, so a resume can apply this rule.
- **Correctness elsewhere is unchanged:** with a strong validator, the `If-Range` rules of #45 still apply, and a `200` is never appended.

## Follow-up

- **Revisit** if the engine's logs show many servers without validators serving files that change at the same length, or if users mistake "not verified" for an error.
