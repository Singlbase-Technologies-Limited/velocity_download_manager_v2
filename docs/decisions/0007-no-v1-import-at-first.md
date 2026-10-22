# ADR-0007: The first V2 releases don't import anything from V1

- **Status:** Accepted
- **Date:** 2026-10-22
- **Decided by:** the Product Owner
- **Issue:** #24

## Context

V2 keeps its own identity and data directory, and never opens V1's database ([ADR-0003](0003-v2-app-identity.md)). V1 stores settings, categories and history in `velocity.sqlite3` (WAL), refuses databases from newer versions, and stores no credentials. Unfinished V1 downloads depend on aria2's `.aria2` control files and partial data, which V2's engine can't safely continue (research report section 6).

## Decision

**No import from V1 in the first V2 releases.** V2 starts clean. The question is revisited after V2 reaches parity (#85).

**Unfinished V1 downloads are never imported,** now or later: their partial data and `.aria2` state are aria2-specific.

## Options considered

| Option | For | Against |
|---|---|---|
| **No import (chosen)** | Simplest and safest; V2 can't touch V1's data | Users re-create their categories and settings |
| Read-only import of settings and categories | Saves users setup time | Mapping work (some settings changed meaning, ADR-0005); a copy of V1's database must be read safely |
| Also history | Users keep their history | More mapping work; history refers to V1 files and records |

## Consequences

- **No V1 reader** is built for the first releases. If import comes later, it reads a **copy** of V1's database, read-only, and never modifies V1.
- **Upgrade guide** (#79) tells V1 users that V2 starts clean, and that unfinished V1 downloads should finish in V1, or be restarted in V2.
- **The installer** (#81) still offers to remove V1. Removing V1 is the user's choice, and V2 never deletes V1's data itself.
