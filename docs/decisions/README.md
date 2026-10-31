# Architecture decision records

An architecture decision record (ADR) states one decision, why it was taken and what follows from it. Records are numbered and never rewritten. A changed decision gets a new record that supersedes the old one.

## Decisions

| ADR | Title | Status | Date |
|---|---|---|---|
| [0001](0001-v2-direction.md) | V2 builds its own direct-download engine; libtorrent for BitTorrent; no aria2 | Accepted | 2026-10-21 |
| [0002](0002-engine-crate-layout.md) | The transfer engine lives in its own crates (`velocity-transfer`, `velocity-curl`) | Accepted | 2026-10-22 |
| [0003](0003-v2-app-identity.md) | V2 keeps its own technical identity, separate from V1's | Accepted | 2026-10-22 |
| [0004](0004-libcurl-build-path.md) | A reviewed git pin of `curl-sys` for the prototype; V2 builds libcurl itself for releases | Accepted | 2026-10-22 |
| [0005](0005-v1-features-without-aria2.md) | What happens to V1 features whose meaning changes without aria2 | Accepted | 2026-10-22 |
| [0006](0006-browser-links-to-local-networks.md) | Browser links to loopback or private addresses need the user's confirmation | Accepted | 2026-10-22 |
| [0007](0007-no-v1-import-at-first.md) | The first V2 releases don't import anything from V1 | Accepted | 2026-10-22 |
| [0008](0008-proxy-support.md) | V2 has full proxy settings in its first release | Accepted | 2026-10-22 |
| [0009](0009-no-aria2-benchmark-gate.md) | Build the engine first; no aria2 benchmark comparison for now | Accepted | 2026-10-22 |
| [0010](0010-resume-without-validator.md) | Without a validator, resume after a length check and show "not verified" | Accepted | 2026-10-22 |
| [0011](0011-sftp-libssh2.md) | SFTP uses libssh2 through libcurl | Superseded by ADR-0013 | 2026-10-22 |
| [0012](0012-engine-child-process.md) | The direct-download engine runs as a supervised child process | Accepted | 2026-10-22 |
| [0013](0013-sftp-russh.md) | SFTP uses russh, in its own crate behind a shared transport interface | Accepted | 2026-10-24 |
| [0014](0014-torrent-helper-child-process.md) | libtorrent runs in a supervised helper process, on a child-process layer shared with the engine | Accepted | 2026-10-26 |
| [0015](0015-keychain-with-consent.md) | Logins are saved in the system keychain only when the user asks, one download at a time | Accepted | 2026-10-29 |
| [0016](0016-release-product-name-v2.md) | The released app is named "Velocity Download Manager V2" | Accepted | 2026-10-30 |
| [0017](0017-gpl-3-licence.md) | V2 is licensed GPL-3.0-only, owned by Singlbase Technologies Limited, with contributions under a CLA | Accepted | 2026-10-31 |

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
