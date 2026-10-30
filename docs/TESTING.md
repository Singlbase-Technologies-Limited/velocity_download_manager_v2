# Testing and verification levels

V2 replaces aria2 with its own transfer engine, so V2's correctness rests on V2's tests. This page is the test strategy (#15):

- **Levels L1–L5:** what each proves, and how long it takes.
- **The test pyramid,** and which issue builds each layer.
- **Fixtures:** the rules, and the local servers V2 needs.
- **Platform reporting:** how results are reported per platform.

All check commands run every check even after one fails, print a PASS/FAIL summary with timings, and exit non-zero if anything failed. They run tools directly, never through a shell.

## Levels and commands

| Level | Command | Scope | Typical duration | When it runs |
|---|---|---|---|---|
| **L1** | `pnpm check:fast` | Whole workspace: the native libraries (`native:build`, reused when up to date; [BUILDING_NATIVE.md](BUILDING_NATIVE.md)), `cargo fmt --check`, `cargo clippy --workspace --all-targets --keep-going -D warnings`, `cargo test --workspace --no-fail-fast`, `tsc --noEmit`, ESLint (zero warnings), Vitest | **Measured:** 5.5–9 s warm, 14 s after a one-file Rust change (Windows; DEVELOPMENT.md). The first run after a clean clone takes 2–3 min | Locally before every PR; in CI on every merge to `main` |
| **L2** | `pnpm test:focused -- <area> [args]` | The tests of one area. A crate runs `cargo test -p <crate> [args]`, unit and integration tests; `frontend` runs `vitest run src`; `scripts` runs `vitest run scripts`; `extension` runs `vitest run browser-extension`. Protocol and fixture suites run here for their crate once they exist | Seconds for today's unit tests; protocol suites will be measured when they arrive | While iterating (Red–Green–Refactor); in CI as part of the selected areas |
| **L3** | `pnpm check:pr [--base <ref>]` | The L1 and L2 checks that the branch's changes need, selected from every path changed since the merge base with `origin/main` | **Measured in CI:** 2 billed min for docs-only, 4–5 for code with a warm cache; locally like L1 | Before pushing; CI's PR tier runs it ([CI.md](CI.md)) |
| **L4** | `pnpm check:full`; later the cross-platform, end-to-end, fuzz, stress and benchmark suites | Every L1 check, then `vite build` and `cargo build --workspace --release` (LTO, one codegen unit), and since #46 the engine's examples for release, so a test-only feature can't break them unseen. `pnpm test:e2e:layout` (#59): the window's layout in a real Chromium, run on its own. Later: browser end-to-end (#64), fuzzing (#73), stress and resource bounds (#77), benchmarks (#20, #21), macOS (#83) | **Measured:** 224–249 s locally, mostly the release build; suites to be measured | Before a release candidate, after build-setting changes, and weekly in CI (#19) |
| **L5** | Production acceptance (none yet) | Installed builds on real machines: install and upgrade beside V1 (#81), parity with V1 (#85), soak runs (research report section 7, at least 300 runs per key scenario), release-readiness review (#78) | Hours to days | Before a release is published |

**In CI** ([CI.md](CI.md)): pull requests run `pnpm check:pr` for the selected areas, and every merge to `main` runs `pnpm check:fast`, `pnpm check:links` and `pnpm check:guidance` on Linux. Every run publishes one report with every failure ([CI_DIAGNOSTICS.md](CI_DIAGNOSTICS.md)).

For documentation, `pnpm check:links` checks that every relative link and heading anchor in the repository's Markdown files resolves. External links aren't fetched.

### Examples

```sh
pnpm test:focused -- velocity-core                 # cargo test -p velocity-core
pnpm test:focused -- velocity-core app_info        # only tests whose name contains app_info
pnpm test:focused -- scripts -t selectChecks       # Vitest, one describe block
pnpm test:watch                                    # Vitest in watch mode
cargo test -p velocity-core -- --nocapture         # cargo directly, with output
pnpm check:pr                                      # what this branch needs
pnpm check:pr --base HEAD                          # only the uncommitted changes
```

Areas for `test:focused` are the workspace crates (`velocity-core`, `velocity-desktop-v2`), `frontend`, `scripts` and `extension` (the browser extension, #64). An unknown area prints the valid ones.

## How `check:pr` selects checks

The rules live in [`scripts/checks-lib.mjs`](../scripts/checks-lib.mjs) and are unit tested in `scripts/checks-lib.test.mjs`. The CI change detection (#13) reuses them, so local and CI selection can't drift apart. The first matching rule wins:

| Changed path | Selects |
|---|---|
| Lockfiles and shared configuration: `Cargo.lock`, `Cargo.toml`, `pnpm-lock.yaml`, `package.json`, `rust-toolchain.toml`, `.gitattributes`, `.editorconfig`, `.gitignore`, `.cargo/`, and `scripts/` (the check machinery) | **Everything** |
| Inside a workspace crate (`crates/<name>/`, `src-tauri/`) | Rust checks for that crate **and every crate that depends on it** (read from `cargo metadata`), plus `cargo fmt` |
| Frontend: `src/`, `public/`, `index.html`, `tsconfig.json`, `vite.config.ts`, `eslint.config.js` | `tsc`, ESLint, Vitest |
| Documentation: `docs/`, root `*.md` | No code checks |
| **Any other path** | **Everything** |

Selecting too little would let a broken change through, so anything shared or unknown selects everything. A new top-level directory needs a rule in `checks-lib.mjs` (with a test) before it can select less.

## Build settings for fast feedback

These settings were measured in #3. The timings are in [DEVELOPMENT.md](DEVELOPMENT.md#check-timings).

- **Dependencies are built with `opt-level = 1` and no debug info.** The app runs at a usable speed, and dependencies are compiled once.
- **Workspace crates are built with `debug = "line-tables-only"`.** Backtraces keep file and line numbers. The Tauri shell's library is also built as a `staticlib` and a `cdylib` for mobile targets. With full debug info, every test run re-linked several hundred megabytes. To debug with variables in a debugger, override it for that build: `CARGO_PROFILE_DEV_DEBUG=true cargo build`.
- **The release profile** (LTO, one codegen unit, `opt-level = "s"`) applies only to `--release` builds and never to development.

Two other settings were measured and rejected:

- **`tsc --incremental`** was slower: 1.3–1.6 s against 0.9 s, because writing the build-info file costs more than it saves on a project this size. Revisit when the frontend grows.
- **ESLint `--cache`** was rejected for correctness. The configuration uses type-aware rules, and ESLint caches results per file. A type change in one file can change the lint result of another file whose cache entry is still valid, so a cached run could pass when a fresh one fails.

## The test pyramid

From most numerous and fastest to fewest and slowest. Each layer proves what the one below can't.

| Layer | What it proves | Tool and location | Level | Built in |
|---|---|---|---|---|
| **Unit** | One function or type in isolation: parsing, validation, naming, status mapping, error classification | Rust `#[cfg(test)] mod tests` beside the code; Vitest `*.test.ts(x)` next to the module | L1 | Every issue |
| **Property** | Invariants over many generated inputs: the scheduler's **I1–I7** (partition, exclusivity, acceptance, monotonic coverage, durability, completion, conservation; research report section 5.2), range arithmetic, header parsing | A seeded generator in the crate's tests. A property-testing crate such as `proptest` needs PO approval first ([DEPENDENCIES.md](DEPENDENCIES.md)) | L1 and L2 | #42, #41, #45 |
| **Protocol fixtures** | The engine against **real libcurl** and a local server that misbehaves on purpose (table below): every row of the research report's section 5.4, checked by SHA-256, the status shown and the wasted bytes | `crates/velocity-transfer/tests/`, `crates/velocity-curl/tests/` ([ADR-0002](decisions/0002-engine-crate-layout.md)) | L2 | #41–#51, #53 |
| **Integration** | The manager, persistence and engine together: lifecycle, queue, pause and resume, checkpoints after `kill -9`, migrations; since #58, every direct-download command on the real supervised engine, the completion gate, a crashed or unavailable engine, and the app starting again (`velocity-transfer` `tests/manager.rs`), and torrents through the manager with a real helper (`velocity-torrent` `tests/manager.rs`) | `crates/velocity-core/tests/`, `crates/velocity-transfer/tests/` | L2 | #26, #43, #50, #58 |
| **UI** | What the user sees and does, against a fake native backend; only the Tauri boundary is mocked | Vitest in jsdom with Testing Library, against Tauri's own `mockIPC` (`@tauri-apps/api/mocks`) through `src/test/fakeBackend.ts` | L1 and L2 | #29, #59–#61 |
| **Layout** | The window in a real browser: dragging the navigation and the list's column edges with a real pointer (a live preview, one save on release), a reload restoring them, and no overflow at the smallest window, in both themes | `pnpm test:e2e:layout` ([`tests/e2e/layout/`](../tests/e2e/layout/layout-e2e.mjs), V1's): Playwright's Chromium against the window with Tauri's IPC mocked (`harness.tsx`), served by Vite. `-- --shots <dir>` saves screenshots. The browser, about 120 MB, comes from `pnpm exec playwright install chromium` once per machine, into the user's Playwright cache, never the repository; `CHROMIUM_PATH` uses another | L4 | #59 |
| **Native host and browser** | Message validation in the host; the real extension, host and app in real Chromium and Firefox | Host: `crates/velocity-native-host/tests/host_e2e.rs`, the real host against a real local listener. Browser: `pnpm test:e2e:chromium` (Playwright's Chromium, Windows and Linux) and `pnpm test:e2e:firefox` (geckodriver, Linux), with V1's extension installed too ([BROWSER_EXTENSION.md](BROWSER_EXTENSION.md#tests)) | L2 (host), L4 (browser) | #64, #65 |
| **Security** | TLS certificate rules per backend, redirect and credential rules, secret-leak scans, filesystem attacks, fuzzed parsers | Fixture suites plus `cargo-fuzz` targets (needs approval) | L2, plus L4 for fuzzing | #67, #69–#73 |
| **End-to-end and platform** | The packaged app installed on Windows, Linux and macOS; install and upgrade beside V1 | Installer tests, real machines | L4 and L5 | #81–#83 |
| **Benchmarks** | V2's own speed, resource use and reliability, measured once the engine and app exist. No aria2 comparison for now ([ADR-0009](decisions/0009-no-aria2-benchmark-gate.md)) | To be designed then | L4 | #77 |

## Fixture policy

- **Local only.** Every test talks to servers it starts itself on `127.0.0.1` (or `::1`), on a free port. No test contacts the public internet, a public tracker, the DHT or a CDN. A test that needs the network is a defect.
- **Seeded data.** Files are generated from a fixed seed with a known SHA-256. Every transfer test checks the SHA-256 of the result, not just "completed".
- **Deterministic timing.** Throttling and delays are controlled by the fixture, never by real network conditions. Timeouts are generous and bounded. A random seed is printed when a test fails.
- **Misbehaviour on purpose.** Fixtures expose switches for the server faults V2 must survive (below), so each test chooses its fault explicitly.
- **No secrets.** Test certificates, keys and passwords are generated for the test or committed as obviously fake fixtures, never real credentials.
- **Real components where it matters.** Fakes are fine at unit boundaries. Claims about the engine, processes, files or crash recovery use real libcurl, real processes and real files.

## Fixture servers V2 needs

| Fixture | Fault switches (research report section 5.4) | Used by | Built in |
|---|---|---|---|
| HTTP/1.1 range server (`crates/velocity-test-http`, standard library only) | **Built:** ranges honoured, Range ignored (`200`), `200` for a non-zero range, malformed `Content-Range`, short `206`, unknown total (`*/`), multipart, cut and overrunning bodies, throttling per connection and per IP (all connections together), configured redirects with a body, `Content-Disposition` (#40), connections closed without an answer, a cut first answer, one byte sent flipped once (`corrupt_once`, #46), and counters for connections, GETs, HEADs, body bytes, `Range` and `If-Range` headers, every request's path and headers (#40), request times and responses sent at once | Probe, scheduler, retries, connections, redirects and file names | **#37** (first modes), extended by #40, #41, #42, #45, #46, #47 and #48 |
| Changed-object modes | **Built** (#45): `ETag`, `Last-Modified` and `Date` sent as configured, `If-Range` honoured as RFC 9110 §13.1.5 says, and a file that changes after a set number of GETs. That one change covers a new strong ETag, a dropped ETag and content changed with the ETag kept. Weak-ETag-only and Last-Modified-only files are configurations | Validators, resume | **#45** |
| Throttle and politeness modes | Speed limits (#49) need no server mode: the server sends as fast as asked, and the engine paces itself. **Built** (#48): `429` and `503` with `Retry-After` on the first requests, a slow but healthy 20 KiB/s stream, a frozen later range. `freeze_process` freezes a whole engine process for the supervisor's hang test | Retries, stall detection, limits | **#48**, #49 |
| Encoding modes | **Built** (#45): `Content-Encoding` sent without encoding anything, so the stored bytes can be compared, and changed between responses with a file change | Range safety | #41, **#45** |
| TLS front with a test CA (`velocity_test_http::tls::TlsFront`, on rustls, ring and rcgen) | **Built:** terminates TLS for the HTTP/1.1 or HTTP/2 server, whose modes and counters work unchanged; a fresh CA per front; leaves that are valid, without an extended key usage, client-auth or code-signing only, from an untrusted issuer, for another host, expired or revoked; a CRL served on a fresh port, so Schannel can check revocation; ALPN `http/1.1` or `h2`; the TLS version and ALPN of each session. Clients trust the CA through velocity-curl's test-only `test-ca` feature ([SECURITY.md](SECURITY.md#how-the-tests-trust-a-test-ca)) | TLS rules, HTTP/2 over TLS | **#69** |
| HTTP/2 over cleartext (`Http2Server` in `crates/velocity-test-http`, on `h2` and tokio) | **Built:** the HTTP/1.1 server's range modes as concurrent streams, throttling per stream or per TCP connection, a slowed first later range, and counters for TCP connections, streams and stream resets. libcurl reaches it with prior knowledge through the `test-h2c` feature of `velocity-curl`, which only test builds have: an optimised build with it doesn't compile (PO decision on #47). The engine's HTTP/2 tests are in the test-only crate `crates/velocity-transfer-h2c`, so `velocity-transfer`'s examples build without the feature (#46) | Multiplexing policy, probe and split over HTTP/2 | **#47** |
| Redirects across origins | Cross-host redirect with credentials and cookies, https→http, redirect to `file:` or `sftp:` | Credential and protocol rules | **#40**, #67, #70 |
| FTP and FTPS (`velocity_test_http::ftp::FtpServer`, standard library, with rustls for FTPS) | **Built** (#51), ported from V1's FTP test server: passive mode (`EPSV`, `PASV`), a required login, `SIZE`, `MDTM`, `REST`, a missing file; switches for no `SIZE`, no `REST`, no `MDTM`, a cut transfer and throttling; a file replaced with another time; explicit FTPS (`AUTH TLS`, `PBSZ`, `PROT P`) and implicit FTPS on a `TestPki` leaf with its revocation list; a record of logins, commands, `RETR`s, `REST` offsets and encrypted connections | FTP, FTPS, resume with `REST`, the cleartext rules | **#51** |
| SFTP (`velocity_test_http::ssh::SshServer`, the installed OpenSSH) | **Built** (#53): the machine's `sshd`, started unprivileged on a free port for the user running the tests, with throwaway host keys (Ed25519, ECDSA or RSA), key logins (one key with a passphrase) and `internal-sftp`; a cut (sshd and its sessions killed) and a restart with the same or a new host key. Without `sshd`, NOT RUN; with `VELOCITY_REQUIRE_SSHD`, as on CI, a failure. **Known flake on Windows** (#62): the unprivileged `sshd`'s SFTP child process sometimes never answers after the login, or ends the session at once. The fetch then fails with a timeout or an interrupted connection, and the test fails. It failed one Windows `check:fast` run on 2026-10-26 and none of the 10 runs of the SFTP tests alone just after; with 4 test binaries at a time it came back 1 to 2 times in 60 runs, and it was worse for a while after heavy repeated load. The server log, which a failing test prints, shows the subsystem starting and no SFTP data. Password logins need a root `sshd`, which CI's Linux job provisions (`scripts/ci-sftp-server.sh`, `VELOCITY_SFTP_PASSWORD_SERVER`), and which WSL gets once through `sudo bash scripts/local-sftp-server.sh` (#176; [platform-verify reference](../.claude/skills/platform-verify/reference.md#local-sftp-password-login-server)); NOT RUN on Windows | Host keys, logins, resume, failures | **#53** (OpenSSH installed by the PO: Windows OpenSSH Server, `openssh-server` in WSL) |
| Proxy (HTTP, SOCKS5) | Credentials never logged | Proxy support | #56 decides whether proxies are in scope |
| Mirrors and Metalink | **Built** (#46): wrong piece hashes, as piece lists the tests supply, with a byte sent flipped once. **Built** (#54): a Metalink corpus (`crates/velocity-core/tests/fixtures/metalink/`: RFC 5854's example, Metalink 4 with subfolders and sources V2 can't use, Metalink 3) with malicious documents in the unit tests (entity bombs, external entities, unsafe and duplicate names, oversized documents), the HTTP server's `Content-Type` (`FileHeaders::content_type`), and an end-to-end test of a followed link whose file gets one bad piece. **Built** (#55): mirrors as several local servers in `crates/velocity-transfer/tests/mirrors.rs`: one missing the file, one dead (a closed port), one with other bytes of the same size, a throttled one beside a fast one, ETags that do or don't match, and the requests a mirror received (no login, no custom header) | Failover, integrity | #46, #54, #55 |
| BitTorrent tracker and peers | Local tracker, libtorrent seeders and leechers; DHT, LPD and peer exchange off | Torrents | **#62** |
| Fake desktop IPC and browser harness | Malformed messages, the app unavailable, wrong caller | Native host, extension | **#65**, #64 |
| Disk and crash faults | Disk full (quota), permission denied, `kill -9` at random points | Recovery | **#74**, #43 |
| Benchmark server | Byte and request counters, throttle modes, h2 and TLS, kill and resume | V2's own benchmarks, later (ADR-0009) | #77 |

## How the engine is tested

- **Scheduler logic without a network.** `velocity-transfer` runs against a **simulated transport**: random chunk sizes, delays, splits, cancellations, retries, duplicate and late callbacks, `200` substitutions and simulated crashes (runtime state dropped, durable coverage kept). After every simulated event, a property test asserts invariants I1–I7 and byte equality with the source.
- **Protocol behaviour with real libcurl.** `velocity-curl` and the engine run against the fixture servers above, so abort-at-boundary, HTTP/2 stream resets and libcurl's real callbacks are covered.
- **Acceptance rows.** Each row of the research report's section 5.4 becomes at least one named fixture test, checking the final SHA-256, the status shown and the wasted bytes.
- **Crash recovery.** `kill -9` at random points over 100 trials: always resumes; re-downloaded bytes are at most one checkpoint interval plus in-flight data (#43, #74).

## The secret-leak scan (#71)

`velocity_test_http::leaks::LeakScan` looks for a test's known secrets everywhere V2 writes or shows something. A test that uses a login, a cookie, a key's passphrase or a tracker passkey runs it at the end:

- **Where it looks:** every file under the folders the test gives it (the database and its WAL, checkpoints, part and finished files), files in the system's temporary folder changed since the test began (three levels deep), texts such as the app's log, the diagnostics export and what the details panel shows, and a running child process's arguments and environment.
- **In what forms:** each secret as written, percent-encoded, and in base64 at each of its three possible alignments, so a `Basic` login or an encoded link is caught too. A secret must be at least 8 bytes, so it can't match by chance.
- **A running child, on Linux only:** it reads `/proc/<pid>/cmdline` and `environ` of the engine and the torrent helper. On Windows that part prints NOT RUN, and `secret_leaks.rs` checks the command a child is started with instead: one fixed argument and a cleared environment (PO decision on #71).
- **Where it runs:** `crates/velocity-transfer/tests/secret_leaks.rs` (HTTP login and cookie and FTP login through the child engine; SFTP key and password logins) and `crates/velocity-torrent/tests/manager.rs` (a private tracker's passkey, through the manager and the torrent helper). The scan's own tests plant each kind of leak and check it is found.

## How the frontend is tested

- **Only the native boundary is mocked:** the UI is rendered in jsdom (the `ui` project in `vite.config.ts`) against a fake backend built on Tauri's own `mockIPC` (`src/test/fakeBackend.ts`), which answers with the contract fixtures and records every call. Events are sent with Tauri's `emit`. Components, hooks and state are real ([UI_ARCHITECTURE.md](UI_ARCHITECTURE.md#testing)).
- **Clicks and typing** go through Testing Library's `user-event`; a drag goes through pointer events. The assertions read roles, names, text and attributes directly, without extra matchers.
- **Contract fixtures:** the shapes the Rust side sends are generated from Rust into `tests/fixtures/contract/` and validated by the UI's strict schemas, so neither side can drift ([CONTRACTS.md](CONTRACTS.md)).
- **What the user sees:** tests check text, roles and actions, not component internals.

## Reporting results per platform

- **Every check, per platform:** PASS, FAIL, BLOCKED or NOT RUN, with the exact command. A pass on one platform says nothing about another (AGENTS.md, "Reporting").
- **NOT RUN needs a reason,** for example "no Mac available" (the PO accepted macOS as NOT RUN for #2) or "OpenSSH not available on Windows".
- **CI results:** reported only after seeing them. A job the selector didn't choose is NOT RUN, not PASS. CI's own statuses (FAILED, INFRA, BLOCKED, CANCELLED…) are explained in [CI_DIAGNOSTICS.md](CI_DIAGNOSTICS.md).
- **Platform runs** use `/platform-verify` (Windows natively, Linux in the WSL clean clone).

## Test-only dependencies

Test tools are dependencies too. Each needs PO approval before it's added ([DEPENDENCIES.md](DEPENDENCIES.md)), recorded in the issue that first needs it. These are expected:

| Tool | For | Issue |
|---|---|---|
| A property-testing crate (for example `proptest`) | Scheduler invariants | #42 |
| `cargo-fuzz` and libFuzzer | Fuzz targets | #73 |
| Testing Library (`@testing-library/react`, `dom`, `user-event`) and jsdom (**approved**, test-only) | UI tests | #29 |
| Playwright (`@playwright/test`, **approved**, test-only) | The layout test (#59); browser end-to-end next | #59, #64 |
| OpenSSH server (**installed**, a platform tool; Windows feature and Ubuntu package) | SFTP fixtures | #53 |
| `h2`, tokio, `http` and `bytes` (**approved**, test-only) | The HTTP/2 fixture server | #47 |
| rustls, tokio-rustls, ring and rcgen (**approved**, test-only) | The TLS fixture | #69 |

Without approval, a hand-written seeded generator or a Node built-in module is the fallback.

## Writing tests

- **Test first:** write the failing test, make it pass with the smallest change, then refactor. Bug fixes get a regression test. `/tdd-change` walks through it.
- **Never weaken a test to get a pass:** don't delete, skip or weaken it.
- **Location:** Rust unit tests live next to the code in `#[cfg(test)] mod tests`. Integration and fixture tests go in a crate's `tests/` directory. Frontend tests are `src/**/*.test.ts(x)`, and script tests are `scripts/**/*.test.mjs`. Names follow [CONVENTIONS.md](CONVENTIONS.md).
- **Determinism:** tests don't use the public network, uncontrolled timing or shared global state (see the fixture policy above).
