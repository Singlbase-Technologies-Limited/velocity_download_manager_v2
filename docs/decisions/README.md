# Architecture decision records

An architecture decision record (ADR) states one decision, why it was taken and what follows from it. Records are numbered and never rewritten. A changed decision gets a new record that supersedes the old one.

## Decisions

| ADR | Title | Status | Date |
|---|---|---|---|
| [0001](0001-v2-direction.md) | V2 builds its own direct-download engine; libtorrent for BitTorrent; no aria2 | Accepted | 2026-10-21 |
| [0002](0002-engine-crate-layout.md) | The transfer engine lives in its own crates (`velocity-transfer`, `velocity-curl`) | Accepted | 2026-10-22 |
| [0003](0003-v2-app-identity.md) | V2 keeps its own technical identity, separate from V1's | Accepted | 2026-10-22 |

Decisions still open are tracked as GitHub issues titled `decision: …`, for example #38 (in-process or child process). [ARCHITECTURE.md](../ARCHITECTURE.md#open-decisions) lists the ones that shape the architecture.

## How a decision is recorded

1. **The question starts as a decision issue.** It lists the options, the verified facts and a recommendation. A recommendation is not a decision.
2. **The Product Owner decides,** in the issue or in conversation. Only the PO accepts an architecture, scope, dependency, licensing or security decision (see [AGENTS.md](../../AGENTS.md)).
3. **The decision is written as the next ADR,** in a PR that links the issue. It is based on [0000-template.md](0000-template.md), and the PR also adds it to the table above.
4. **Documents that depend on the decision are updated in the same PR,** for example [ARCHITECTURE.md](../ARCHITECTURE.md) and the glossary.

**Status values:**

- **Proposed:** written, not yet accepted.
- **Accepted:** in force.
- **Superseded by ADR-NNNN:** replaced; kept for history.
- **Rejected:** considered and declined; kept so the question is not reopened without new evidence.

**File names:** `NNNN-short-title.md`, numbered in order and never reused.

Some workflow and identity decisions made before this folder existed are recorded as issue comments and in the docs that apply them:

- The main-centred workflow is in [DEVELOPMENT_WORKFLOW.md](../DEVELOPMENT_WORKFLOW.md).
- The agent account and the decision to defer branch protection are in #89 and [AI_AGENT_ACCOUNT.md](../AI_AGENT_ACCOUNT.md).
- The product name is in #11.
