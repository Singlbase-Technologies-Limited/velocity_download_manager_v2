# Documentation map

Every document in `docs/` is listed here. A new document is added to this map in the same PR. At the repository root, [CONTRIBUTING.md](../CONTRIBUTING.md) is the path from clone to pull request, and [AGENTS.md](../AGENTS.md) holds the rules for contributors and AI agents.

## Start here

| Document | Read it to |
|---|---|
| [DEVELOPMENT.md](DEVELOPMENT.md) | Set up a clean clone, run the app, and see the commands and build times |
| [TROUBLESHOOTING.md](TROUBLESHOOTING.md) | Fix common setup and build problems |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Understand the components, who owns which state, and the open architecture decisions |
| [GLOSSARY.md](GLOSSARY.md) | Look up a term (lease, coverage, validator, checkpoint, capability and more) |
| [CONVENTIONS.md](CONVENTIONS.md) | Name Rust and TypeScript code, files, tests, commands, events and error codes |

## How we work

| Document | Read it to |
|---|---|
| [DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md) | Follow the branch, commit, pull-request, merge and release-tag workflow |
| [REVIEWING.md](REVIEWING.md) | Review a pull request |
| [TESTING.md](TESTING.md) | Choose the right verification level and learn how `check:pr` selects checks |
| [DEPENDENCIES.md](DEPENDENCIES.md) | Propose, approve, pin and verify a dependency; the allowed licences and the provenance release blockers |
| [LICENSING.md](LICENSING.md) | Read the licensing assessment: today's third-party licences, planned native libraries, notices |
| [CI.md](CI.md) | See what CI runs on each event, how jobs are selected, cached and secured, and the monthly cost model |
| [CI_DIAGNOSTICS.md](CI_DIAGNOSTICS.md) | Read a failed CI run: the statuses, the `ci-diagnostics` report, INFRA versus FAIL, and the re-run policy |
| [AI_AGENT_ACCOUNT.md](AI_AGENT_ACCOUNT.md) | Find out which GitHub account an AI agent uses, and how to check it |

## Decisions and research

| Document | Read it to |
|---|---|
| [decisions/README.md](decisions/README.md) | See the index of accepted decisions and how a decision is recorded |
| [decisions/0000-template.md](decisions/0000-template.md) | Start a new decision record |
| [decisions/0001-v2-direction.md](decisions/0001-v2-direction.md) | Read ADR-0001: own engine on libcurl, libtorrent for BitTorrent, no aria2 |
| [decisions/0002-engine-crate-layout.md](decisions/0002-engine-crate-layout.md) | Read ADR-0002: the engine lives in `velocity-transfer` and `velocity-curl` |
| [decisions/0003-v2-app-identity.md](decisions/0003-v2-app-identity.md) | Read ADR-0003: V2's own identifiers (app ID, native host, extension IDs) |
| [decisions/0004-libcurl-build-path.md](decisions/0004-libcurl-build-path.md) | Read ADR-0004: how libcurl is obtained for the prototype and for releases |
| [research/README.md](research/README.md) | See the research index; research is input, not decisions |
| [research/2026-10-21-libcurl-engine-research-r2.md](research/2026-10-21-libcurl-engine-research-r2.md) | Read the libcurl download-engine research report, revision 2 |

`images/` holds screenshots that the pages above embed.
