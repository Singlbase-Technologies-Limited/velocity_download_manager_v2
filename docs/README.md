# Documentation map

Every document in `docs/` is listed here. A new document is added to this map in the same PR. At the repository root, [CONTRIBUTING.md](../CONTRIBUTING.md) is the path from clone to pull request, and [AGENTS.md](../AGENTS.md) holds the rules for contributors and AI agents.

## Start here

**Using V2?** The [user guide](user/README.md) covers installing, downloads, protocols and their limits, torrents, categories, settings, browser integration, troubleshooting, and using V2 beside V1 (#79). The documents below are for people who build V2.

| Document | Read it to |
|---|---|
| [DEVELOPMENT.md](DEVELOPMENT.md) | Set up a clean clone, run the app, and see the commands and build times |
| [TROUBLESHOOTING.md](TROUBLESHOOTING.md) | Fix common setup and build problems |
| [PARITY.md](PARITY.md) | See every V1 feature (commands, settings, views, engine behaviours, extension, persistence) with its V1 code and tests, V2 disposition and V2 issue |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Understand the components, who owns which state, and the open architecture decisions |
| [GLOSSARY.md](GLOSSARY.md) | Look up a term (lease, coverage, validator, checkpoint, capability and more) |
| [COEXISTENCE.md](COEXISTENCE.md) | See how V2 stays separate from V1: identity, data folder, single instance, IPC names |
| [BROWSER_EXTENSION.md](BROWSER_EXTENSION.md) | Work on the browser extension: what it intercepts, its rules and fallbacks, V2's extension IDs and key, the protocol, permissions, building and tests |
| [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md) | Change how the manager talks to a direct-download engine: the `DirectTransfer` commands, events, secrets and wire forms |
| [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md) | Change how the engine fetches a file: the probe, the range scheduler and its invariants, anomalies, wasted bytes and disk writes |
| [ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md) | Change how the app and its child processes (the engine and the torrent helper) talk: framing, versions, limits, the supervisor and its restarts |
| [TORRENTS.md](TORRENTS.md) | Work on torrents: the bridge, the torrent helper, the checks before libtorrent, the torrent contract and statuses, seeding, resume data and recovery, deleting a torrent's files, trackers and peers, settings and port mapping, and what is still to come |
| [DATA_AND_MIGRATIONS.md](DATA_AND_MIGRATIONS.md) | Change the app database: the file, the schema, unknown values, secrets, and how to add a migration |
| [DIAGNOSTICS.md](DIAGNOSTICS.md) | See what V2 logs, how secrets are redacted, what the diagnostics export holds, and the crash-log policy |
| [PRIVACY.md](PRIVACY.md) | See what V2 sends (only the downloads), what it keeps on this computer, and what a diagnostics export holds |
| [SECURITY.md](SECURITY.md) | See how V2 verifies TLS certificates on each platform, what was measured (purpose, host, expiry, revocation, TLS 1.3), and how the tests trust a test CA |
| [SETTINGS.md](SETTINGS.md) | See each setting V2 has so far, what it does and its bounds: performance profiles, the queue, folders, and what a build can do |
| [FILE_NAMING.md](FILE_NAMING.md) | Change how a download's file is named, its category and folder are chosen, and what happens when the name is taken |
| [ACCESSIBILITY.md](ACCESSIBILITY.md) | See what V2 does for keyboard and screen-reader users, how each behaviour is tested, the contrast rules, and the manual screen-reader check |
| [UI_ARCHITECTURE.md](UI_ARCHITECTURE.md) | Change the window: where its state lives, the command bridge, starting up, the shell, the tray and notifications, styling and UI tests |
| [CONTRACTS.md](CONTRACTS.md) | Change a type shared by the UI and Rust: the domain types, contract fixtures and the contract version |
| [CONVENTIONS.md](CONVENTIONS.md) | Name Rust and TypeScript code, files, tests, commands, events and error codes |

## How we work

| Document | Read it to |
|---|---|
| [DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md) | Follow the branch, commit, pull-request, merge and release-tag workflow |
| [REVIEWING.md](REVIEWING.md) | Review a pull request |
| [TESTING.md](TESTING.md) | The test strategy: levels L1–L5 with durations, the test pyramid, fixture policy and servers, and how `check:pr` selects checks |
| [DEPENDENCIES.md](DEPENDENCIES.md) | Propose, approve, pin and verify a dependency; the allowed licences and the provenance release blockers |
| [LICENSING.md](LICENSING.md) | Read the licensing assessment: today's third-party licences, planned native libraries, notices |
| [PACKAGING.md](PACKAGING.md) | Build the Windows installer and the Linux packages, see what they install and how they live beside V1, and test install, upgrade and uninstall |
| [OPEN_SOURCE.md](OPEN_SOURCE.md) | See who decides, the community files, the audit before going public and the checklist for it |
| [RELEASE_PROCESS.md](RELEASE_PROCESS.md) | Change V2's version, draft and edit the changelog ([`CHANGELOG.md`](../CHANGELOG.md)), and tag a candidate or a release |
| [RELEASE_CANDIDATES.md](RELEASE_CANDIDATES.md) | Build a release candidate from a verified `main` commit, see what it holds, check it, install it beside V1, and go back to an earlier version |
| [BUILDING_NATIVE.md](BUILDING_NATIVE.md) | Build the native libraries (libcurl, nghttp2, zlib, OpenSSL) from verified sources, read `build-info.json`, update a pin, and build libtorrent with vcpkg |
| [CI.md](CI.md) | See what CI runs on each event, how jobs are selected, cached and secured, and the monthly cost model |
| [CI_DIAGNOSTICS.md](CI_DIAGNOSTICS.md) | Read a failed CI run: the statuses, the `ci-diagnostics` report, INFRA versus FAIL, and the re-run policy |
| [AGENT_GUIDANCE.md](AGENT_GUIDANCE.md) | See how AGENTS.md, CLAUDE.md, rules and skills are organised, what `pnpm check:guidance` checks, and the monthly prompt audit |
| [AI_AGENT_ACCOUNT.md](AI_AGENT_ACCOUNT.md) | Find out which GitHub account an AI agent uses, and how to check it |
| [REPOSITORY_SETTINGS.md](REPOSITORY_SETTINGS.md) | See each GitHub setting of this repository, its value, why, and a read-only command that checks it |

## Decisions and research

| Document | Read it to |
|---|---|
| [decisions/README.md](decisions/README.md) | See the index of accepted decisions and how a decision is recorded |
| [decisions/0000-template.md](decisions/0000-template.md) | Start a new decision record |
| [decisions/0001-v2-direction.md](decisions/0001-v2-direction.md) | Read ADR-0001: own engine on libcurl, libtorrent for BitTorrent, no aria2 |
| [decisions/0002-engine-crate-layout.md](decisions/0002-engine-crate-layout.md) | Read ADR-0002: the engine lives in `velocity-transfer` and `velocity-curl` |
| [decisions/0003-v2-app-identity.md](decisions/0003-v2-app-identity.md) | Read ADR-0003: V2's own identifiers (app ID, native host, extension IDs) |
| [decisions/0004-libcurl-build-path.md](decisions/0004-libcurl-build-path.md) | Read ADR-0004: how libcurl is obtained for the prototype and for releases |
| [decisions/0005-v1-features-without-aria2.md](decisions/0005-v1-features-without-aria2.md) | Read ADR-0005: profiles, disk settings, Metalink, engine restart and other aria2-era features in V2 |
| [decisions/0006-browser-links-to-local-networks.md](decisions/0006-browser-links-to-local-networks.md) | Read ADR-0006: browser links to your PC or local network need confirmation |
| [decisions/0007-no-v1-import-at-first.md](decisions/0007-no-v1-import-at-first.md) | Read ADR-0007: no import from V1 in the first releases |
| [decisions/0008-proxy-support.md](decisions/0008-proxy-support.md) | Read ADR-0008: full proxy settings in the first release |
| [decisions/0009-no-aria2-benchmark-gate.md](decisions/0009-no-aria2-benchmark-gate.md) | Read ADR-0009: build the engine first, with no aria2 benchmark comparison for now |
| [decisions/0010-resume-without-validator.md](decisions/0010-resume-without-validator.md) | Read ADR-0010: without a validator, resume after a length check and show "not verified" |
| [decisions/0011-sftp-libssh2.md](decisions/0011-sftp-libssh2.md) | Read ADR-0011: SFTP uses libssh2 through libcurl (superseded by ADR-0013) |
| [decisions/0013-sftp-russh.md](decisions/0013-sftp-russh.md) | Read ADR-0013: SFTP uses russh, in its own crate behind a shared transport interface |
| [decisions/0012-engine-child-process.md](decisions/0012-engine-child-process.md) | Read ADR-0012: the direct-download engine runs as a supervised child process |
| [decisions/0014-torrent-helper-child-process.md](decisions/0014-torrent-helper-child-process.md) | Read ADR-0014: libtorrent runs in a supervised helper process, on a child-process layer shared with the engine |
| [research/README.md](research/README.md) | See the research index; research is input, not decisions |
| [research/2026-10-21-libcurl-engine-research-r2.md](research/2026-10-21-libcurl-engine-research-r2.md) | Read the libcurl download-engine research report, revision 2 |
| [research/engine-prototype-results.md](research/engine-prototype-results.md) | Read what the engine prototype (#37) showed, in both modes, as evidence for #38 |
| [research/engine-mode-costs.md](research/engine-mode-costs.md) | Read what running the engine as a child process costs, measured for #38 |

`images/` holds screenshots that the pages above embed.
