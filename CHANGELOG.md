# Changelog

What changed in each version of Velocity Download Manager V2. Versions follow [Semantic Versioning](https://semver.org/); while V2 is below 1.0.0, a minor version may change things that break. How sections are drafted from the merged pull requests, and edited into release notes, is in [docs/RELEASE_PROCESS.md](docs/RELEASE_PROCESS.md).

No version has been released yet. The section below lists everything merged so far, as `pnpm changelog start` drafts it.

## Unreleased

### Features

- add engine-neutral domain types, error codes and the checked UI contract (#25, PR #123)
- give V2 its own identity, data folder, single instance and IPC names (#27, PR #124)
- add the engine-neutral DirectTransfer contract, a fake engine and the manager's first slice (#36, PR #126)
- **curl:** link the Velocity-built libcurl and refuse to start one that fails the capability check (#35, PR #128)
- **curl:** one libcurl Multi loop with a gate, bounded buffers and safe defaults (#37, part 1) (PR #139)
- **transfer:** the engine prototype in process and as a child process, with its report (#37, PR #140)
- **transfer:** a framed, versioned engine protocol and a supervisor with capped restarts (#39, PR #143)
- **transfer:** the production range probe, sequential ranges and anomaly handling (#41, PR #144)
- **transfer:** the range scheduler with exclusive leases, splits and property-tested invariants (#42, PR #145)
- **core:** the SQLite app database with versioned migrations, backups and unknown-value handling (#26, PR #146)
- **transfer:** checkpoints, crash recovery, pause and resume, sparse part files and a no-replace finish (#43, PR #148)
- **transfer:** retries with backoff and jitter, Retry-After, stall detection, whole-download restarts and hung-engine restarts (#48, PR #149)
- **transfer:** global and per-download speed limits with token buckets, and measured disk back-pressure (#49, PR #151)
- **transfer:** adaptive connections, per-host and global budgets, HTTP/2 multiplex policy and fairness (#47, PR #152)
- **transfer:** changed-object detection with strong validators and If-Range, unasked encodings kept, expired links (#45, PR #153)
- **transfer:** whole-file integrity: checksums, hash frontier, piece hashes and verification state (#46, PR #156)
- **core:** Velocity's own download queue: concurrency, order, schedule window, add-paused and live settings (#50, PR #158)
- performance profiles as engine limits, capabilities per protocol, and their UI explanations (#57, PR #164)
- HTTP metadata: request options, redirects Velocity follows itself, and file names from the server (#40, PR #165)
- FTP and FTPS: passive transfers, logins, REST resume against MDTM, explicit and implicit FTPS (#51, PR #170)
- SFTP on russh with strict host keys, key and password logins, and resume (#53, PR #171)
- Metalink 3 and 4: safe parsing, file selection in subfolders, followed links, trusted piece hashes (#54, PR #178)
- mirrors: several sources at once with health scoring, failover, and no unproven mixing (#55, PR #179)
- torrents on libtorrent 2.0.15: the bridge, the torrent helper, torrent checks and a shared child-process layer (part of #62) (PR #180)
- magnets checked before anything downloads, and file choice in the torrent helper (part of #62) (PR #181)
- torrents seed under policies, come back from resume data after a restart or crash, and delete only their own files (part of #62) (PR #182)
- torrent trackers, re-announce, peers, limits, the listening port and opt-in port mapping (closes #62) (PR #183)
- the download manager on the V2 engine: saved downloads, recovery, retry, removal, engine loss, and the app's downloads commands (part of #58) (PR #185)
- torrents in the download manager: the helper without libtorrent in the app, the shared queue, storage without trackers, recovery and deletion (closes #58) (PR #186)
- categories, automatic folders, file naming, and taken names asked about or renamed, never replaced (closes #31) (PR #187)
- the app shell: sidebar, status bar, theme, saved layout, first-run setup, Categories screen, tray, close to tray, exit confirmation and notifications (#29, PR #189)
- the Add download dialog: links, FTP and SFTP, magnets, .torrent files and links, duplicates, destination checks and the SFTP host-key question (closes #60) (PR #190)
- **ui:** the download list, with the retry countdown, Started and Avg. speed, and the login asked again (#59, PR #191)
- the details panel follows the selection, with connections, errors, torrent activity, options, and opening files (#61, PR #193)
- torrent workflows in the UI: choose files, seeding and upload limits, trackers, peers and re-announce (#63, PR #194)
- the History view, with retention by days and count, clearing and deleting entries that leave the files alone (#32, PR #195)
- logging with redaction, diagnostics with export, and local crash logs (#28, PR #196)
- the Settings screen and model, with every engine setting working and each section saved whole (#30, PR #197)
- **extension:** V2's own browser extension for Chromium and Firefox, apart from V1's (#64, PR #198)
- V2's native messaging host and the app's browser bridge, registered per browser beside V1's (#65, PR #200)
- **a11y:** the accessibility audit with automated checks, text contrast to WCAG AA, and focus back after dialogs (#66, PR #201)
- proxy support for every download, torrents included, and the Settings screen's Proxy section (#192, PR #212)

### Fixes

- name dev builds "Velocity Download Manager (dev)", record the Linux run and document the agent account (part of #2) (PR #93)
- **transfer:** bound the segment queue and a test-server race; measure the engine modes; ADR-0012 child process (#38, PR #142)
- **scripts:** check:pr-body checks a description without --title (#203, PR #207)

### Security

- add a PreToolUse agent guard that blocks pushes to main, force pushes and reset --hard (PR #129)
- redirects only to http and https, https to http and browser links to local addresses confirmed, and links normalised (#67, PR #199)
- secrets wiped from memory, logins remembered in the system keychain on consent, and a leak test (#70, PR #202)
- shared file-safety checks, long paths, look-alike names and link-safe removal (#72, PR #209)
- an advisory check with cargo-deny and pnpm audit, response times and an advisory register (#75, PR #211)

### Documentation

- define the main-centred workflow and the review checklist (#6, PR #94)
- add the architecture overview, glossary, decision records and documentation map (#8, PR #96)
- add the V2 contributor and agent rules, Claude Code operating rules and guardrails (#4, PR #97)
- add the CI design with tiers, selection, caching, security and a cost model (#12, PR #103)
- record the engine crate layout and V2's own identity as ADR-0002 and ADR-0003 (#33, #11) (PR #116)
- add the dependency policy and the licensing assessment (#9, PR #117)
- record the libcurl build path as ADR-0004 (#34, PR #118)
- define the test strategy, pyramid, fixture policy and levels L1-L5 (#15, PR #119)
- add the evidence-based V1 feature inventory and V2 parity map (#22, PR #120)
- record the decisions on V1 features without aria2 as ADR-0005 (#23, PR #121)
- record decisions on local-network links, V1 import and proxies as ADR-0006 to ADR-0008 (#68, #24, #56) (PR #122)
- fix the findings of the first prompt audit (PR #132)
- ADR-0009 to ADR-0011, no aria2 benchmark gate, resume without a validator, SFTP through libssh2 (PR #138)
- record zod 4.6.6 in the dependency register (#162, PR #163)
- repository settings, their values and read-only checks (#7, PR #217)
- engine, contract and data documents checked against the code (#80, PR #219)

### Packaging and builds

- bootstrap the V2 workspace with a documented clean-clone dev build (#2, PR #92)
- add L2-L4 check commands, path-based check selection and faster dev builds (#3, PR #95)
- **native:** build libcurl from verified sources with a build identity record (#35, part 1) (PR #127)
- **deps:** Bump zod from 4.6.5 to 4.6.6 (PR #160)
- third-party notices generator and licence check (#76, PR #218)
- Windows installer, per-user NSIS, as the release identity (#81, PR #220)

### Continuous integration

- add change-to-test job selection and the PR workflow with fail-safe defaults (#13, PR #104)
- cache Rust dependencies, add main checks that save the cache, and Dependabot for actions (#14, PR #105)
- report nothing for superseded runs and record the measured CI timings (#14, PR #109)
- complete every check and publish one consolidated failure report (#91, PR #110)
- bound the apt-get step and fall back from a stalled Ubuntu mirror (#135, PR #136)
- post-merge checks on main with Windows jobs and a tracking issue for a red main (#16, PR #134)
- local verification while CI is paused, with a local password-login SFTP server (#176, PR #177)
- run CI only by hand, once per batch of five issues, temporarily (#204, PR #213)
- build libtorrent with vcpkg in CI, and cache the native libraries on their own (#205, PR #214)

### Tests

- check the agent guidance files and record the first prompt audit (PR #131)
- **transfer:** steady #47's connection timing fixtures on slow Linux machines (#154, PR #155)
- **transfer:** give the per-IP ramp fixture a noisy window's headroom (#159, PR #161)
- **tls:** TLS verification per backend on a local test CA with served revocation, and the Windows store path (#69, PR #166)
- **http2:** give the per-connection throttle fixture headroom above stream saturation (#168, PR #169)
- run the speed-limit tests one at a time (#174, PR #175)
- a shared secret-leak scan over files, temp files, logs, the diagnostics export and running children (#71, PR #208)
- UI tests wait up to 3 s for what they look for (#206, PR #215)
- a full disk, a refused write and a killed app resume without losing what was downloaded (#74, PR #210)
- the snapshot timing test waits for the last change, not a fixed window (#188, PR #216)

### Other changes

- bootstrap repository with line-ending policy and README (PR #2)
- add six project skills for the V2 issue workflow (#87, PR #99)
- add V2 issue forms and the pull request template (#5, PR #100)
- apply the decision label from the decision issue form (#101, PR #102)
