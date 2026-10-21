> **Research, not decisions.** This is a verbatim copy of revision 2 of the research report, dated 2026-10-21. It was exported as Markdown from the Claude Doc <https://claude.ai/artifact/NYyKUpBeoMLipdarEicJD2> on 2026-10-21 (doc revision 38).
>
> - **About V1:** the report describes the original Velocity Download Manager (V1) at `e9d717a8`. Paths such as `manager.rs` or `vendor/aria2/` refer to that repository, not to V2.
> - **Overridden for V2:** where the report keeps aria2 as a default, legacy or fallback engine, the Product Owner's direction overrides it. V2 contains no aria2. See [ADR-0001](../decisions/0001-v2-direction.md) for what does and does not carry over.
> - **Decisions:** the report's recommendations are inputs to the decision issues, not decisions. Accepted decisions live in [docs/decisions/](../decisions/README.md).
> - **Export loss:** one embedded diagram did not survive the export. It appears below as "embedded content: proposed engine architecture" in section 4. The text around it describes it.

# Velocity libcurl download engine — research report

Oct 21, 2026 · @Vincent

**Revision 2, 2026-10-21.** This revision independently re-checks seven audit findings (R1 to R7) and corrects revision 1 where the evidence required it. Corrections are listed in "What changed in revision 2", with their sources. Labels used throughout: **VERIFIED** (read in code, an official document or source, or observed), **INFERRED** (reasoned from verified facts), **NOT TESTED** (needs a prototype or a test run) and **DEFERRED** (deliberately left for later).

## Executive summary

**Approve now:** stage 1 (a small engine-neutral seam plus a forward-compatibility fix to the database), stage 2 (benchmark harness v2, including native Windows measurement) and stage 3 (a disposable libcurl HTTP(S) prototype built on **libcurl 8.23.0 or later**). **Do not approve yet:** the replacement engine, SFTP, the production deployment model (in-process or child process) or any default change. These depend on the prototype meeting the gates in section 7. aria2 stays the default and keeps every download it already owns.

- **Why change at all.** The vendored aria2 has the same gaps as before: HTTP/1.1 only, no validator-checked resume, no TLS 1.3 on Windows, no SFTP in Velocity's builds, and a restart whenever the connection count changes (VERIFIED, section 1). None of these is a *measured* speed gap.
- **Why libcurl remains the lead candidate.** The brief now names SFTP as a required protocol. libcurl is the only permissively licensed stack that covers HTTP/1.1, HTTP/2, FTP, FTPS, SFTP and every proxy type, and has the per-host connection controls segmentation needs. reqwest would need two more protocol stacks. It stays only as an HTTP fallback if the libcurl prototype fails its gates (INFERRED).
- **Release blockers found in revision 2 (VERIFIED):**
  - The proposed `curl-sys` 0.4.91 bundles exactly curl 8.22.0. Its submodule is the `curl-8_22_0` tag commit `01346829`. curl lists 21 advisories whose last affected version is 8.22.0, including **CVE-2026-92392** (High, HTTP/2 upload use-after-free) and **CVE-2026-87865** (libssh2 known-hosts mismatch reported as "missing"). The update to 8.23.0 is an open, unmerged PR (curl-rust #692).
  - `curl-sys`'s bundled Linux build compiles libcurl with `select()` instead of `poll()`, so connections fail once a descriptor number reaches 1024 (curl-rust issue #684, fix PR #685 open).
  - A drop-order segfault fix (#645) was merged after the 0.4.51 release.
- **Design corrections:**
  - **Probe:** a bounded probe replaces `Range: bytes=0-`.
  - **Segments:** work-stealing now uses exclusive leases with ids, and stale writers are rejected.
  - **Hashing:** "incremental hashing removes the verification pass" was wrong for out-of-order segments. SHA-256 values of pieces cannot be combined.
  - **Benchmark gates:** rev 1 asked for a 10% gain even where equal connection counts make one physically impossible. The new gates require non-inferiority against the *best valid* aria2 configuration, and treat any corrupted file as an absolute blocker.
  - **Rollback:** today's code reads any unknown `engine` value as aria2 (`database/downloads.rs:139`), so rev 1's rollback claim was wrong.
- **Deployment.** Revision 1 preferred running in-process. Revision 2 recommends a **contract that supports both** models, with a supervised child process as the production target unless the prototype shows a material cost. Release builds use `panic = "abort"`, and the engine would host C TLS, HTTP/2 and SSH parsers. This is a Product Owner decision (section 10).
- **Licensing.** The "no copyleft" claim holds only for the *static* configurations listed in section 8. A distro dynamic libcurl brings libssh and librtmp (both LGPL-2.1), plus libidn2: the saved benchmark's `curl -V` line shows them (VERIFIED). GPL obligations continue for as long as any installer ships aria2.
- **Nothing in this report has been built, installed or benchmarked.** All performance claims about the proposed engine are NOT TESTED.

## What changed in revision 2

| # | Revision 1 said | Revision 2 finds | Evidence | Label |
| --- | --- | --- | --- | --- |
| R1 | `curl-sys` bundles 8.22.0; curl 8.23 exists | 8.22.0 is affected by 21 advisories fixed in 8.23.0. The bundled path is blocked until upgraded, and the `select()` bug must be fixed. A version floor and identity policy are added. | Section 8, curl advisories, curl-rust #692/#684/#685/#645 | VERIFIED |
| R2 | Probe with `Range: bytes=0-` | Bounded first-segment probe. A `200` at offset 0 becomes the single stream (nothing wasted). A `200` at a non-zero offset is aborted in the header callback before any byte is written. | Section 5.1, RFC 9110 §§14.2, 15.3.7 | VERIFIED rules, NOT TESTED design |
| R3 | 10% faster with a 95% CI; Windows `--platform-sampler` | Non-inferiority against the best valid aria2 configuration; superiority only where a mechanism exists. The sampler **does not exist**: the harness reads `/proc` only and runs curl through `sh -c`. | Section 7, `scripts/bench/run-benchmarks.mjs` | VERIFIED |
| R4 | Split the largest in-flight range | Exclusive leases with ids, a single owner of range accounting, bounded acceptance of writes, and durable coverage only after fsync. Invariants I1 to I7 and property tests. | Section 5.2 | NOT TESTED design |
| R5 | ETag/`If-Range`; incremental hashing | Strong ETag only. Last-Modified only when strong by RFC 9110 §8.8.2.2. Never append a `200`. Pieces need trusted per-piece hashes. The whole-file hash needs a sequential pass. | Section 5.3, RFC 9110 §§8.8, 13.1.5 | VERIFIED rules |
| R6 | In-process, like libtorrent | Both models are compared on Velocity's real boundaries. A transport-neutral contract; a child process over stdio is the recommended production target. | Section 4.3, workspace `Cargo.toml` `panic = "abort"` | INFERRED |
| R7 | No copyleft without SFTP and HTTP/3 | Per-platform inventory with versions. True only for the static builds listed. The distro libcurl closure, the AppImage risk and the GPL exit conditions are added. | Section 8 | VERIFIED versions, INFERRED closure |
| — | Old builds ignore new `engine` values | Wrong: unknown values become `aria2`. Safe only because a newer schema is refused (`database/mod.rs:65`). Fix the parser first. | Section 6 | VERIFIED |
| — | SFTP is advanced parity | Now **required** per the brief. The plan still gates it behind a backend choice. | Sections 3, 9, 10 | — |

## 1. Findings about current Velocity

All of the following was read from `develop` at `e9d717a8b96ef8fa21ebba40f5965ae9fc6d42e7` on 2026-10-21. A live `git fetch` showed that `origin/develop` is still at that commit, and that the working tree's only change is an untracked `.agents/` folder (VERIFIED).

*Revision 2 re-check (2026-10-21):* `git fetch` shows that `origin/develop` is still `e9d717a8b96ef8fa21ebba40f5965ae9fc6d42e7`, committed 2026-10-20 21:44 +0300. The only open issues are #4, #5, #9, #11 to #18 and #35 to #46. None proposes an HTTP engine change. Issue #18 (the vendored aria2 as the Windows default) still awaits acceptance. The report's comment thread had no reply. The brief for this revision makes SFTP a required protocol (VERIFIED).

### What issue #35 is, and what it is not

Issue #35 is the UX and torrent-workflow epic (#36 to #46, merged through PR #47). It does **not** ask for a new HTTP engine. It says to keep aria2 for non-torrent transfers. No open or closed issue proposes replacing aria2 for HTTP. The nearest existing decision is `docs/ARIA2_FORK_STRATEGY.md`, Phase 5. It allows Rust replacements "at stable capability boundaries, starting with an isolated HTTP/HTTPS path" behind "an explicit engine contract", one capability at a time, gated by benchmarks (VERIFIED). This research therefore extends Phase 5. It does not reverse an accepted decision. Issue #18, which makes the vendored aria2 the Windows default, is still awaiting Product Owner acceptance.

### Current architecture (VERIFIED from code)

| Area | Where | What it does today |
| --- | --- | --- |
| Engine boundary | `crates/velocity-core/src/engine/router.rs` | `EngineRouter` exposes **aria2's operations**: `add_uri`, `tell_status`, `change_option`, `change_position`, `poll` returning `Aria2Status`. A GID beginning `lt` goes to libtorrent; everything else goes to aria2. |
| libtorrent | `crates/velocity-lt` (cxx shim), `velocity-core/src/libtorrent/engine.rs` | Runs in the app process. **Imitates aria2**: it returns `Aria2Status`/`Aria2File` shapes so the manager has one code path. |
| aria2 sidecar | `engine/mod.rs`, `aria2/rpc.rs`, `engine/config.rs` | A supervised child process using JSON-RPC on 127.0.0.1 with a 256-bit secret per launch, a 0600 launch config, and restart backoff (at most five restarts in five minutes). The session is saved by the app every 15 s, then scrubbed of credentials. |
| Manager | `manager.rs` (2,923 lines, 73 aria2 references), `manager/torrent.rs` | Add, submit, poll, reconcile and finalize. App-level retries (transient, throttled or permanent; 429/503 wait at least 60 s). Re-adds a lost GID after three missed polls. |
| Optimizer | `optimizer.rs` | Only **chooses aria2 options**: `split`, `max-connection-per-server` and `min-split-size` per profile. It halves connections on throttled retries, and has an opt-in one-time step-up. `ARIA2_MAX_CONN_PER_SERVER = 16`. |
| Validation | `validation.rs` (838 lines) | Scheme allowlist (SFTP refused at line 75), IDN handling, credentials moved out of URLs, header denylist (`cookie`, `proxy-authorization`…), checksum `algo=hex`, filename and extension rules, `safe_conf_value`. |
| Persistence | `database/`, `migrations/0001`–`0003` | SQLite (WAL, 0600), versioned migrations with a `VACUUM INTO` backup first. `downloads` stores `gid`, `engine` (`aria2`, `libtorrent`) and `options_json` (aria2 option names). Credentials and header values are **never stored**. |
| Finalization | `manager.rs:2250` `finalize_complete` | Marks a download Completed only when every selected file exists with the **expected length**. A content hash is checked only when the user supplied a checksum, and aria2 checks it. |
| UI contract | `src-tauri/src/commands.rs`, `src/lib/api.ts`, Zod in `src/types/models.ts` | Typed commands and a `velocity://snapshot` event. 23 references to `aria2` in `src/`, mostly in diagnostics and copy. |

### How tightly the manager is coupled to aria2 (measured in revision 2)

The manager is coupled to aria2 through about **55 call sites** of `EngineRouter` in `manager.rs` and `manager/torrent.rs`. The most frequent are `remove_download_result` (13), `force_remove` (8), `tell_status` (7) and `change_option` (5). There are also **18 uses of `Aria2Status`** and aria2 option names written directly by the manager: `split`, `max-connection-per-server`, `out`, `http-user`, `ftp-user`, `checksum`, `allow-overwrite`, `select-file` and others. This was counted with grep at `e9d717a8` (VERIFIED). A full generic engine framework is therefore a large refactor. Section 4.2 proposes a smaller seam that only direct transfers use. Torrent-only operations (`reannounce`, `trackers`, `get_peers`, `select-file`, seeding) stay on the existing torrent path.

### What Velocity owns, and what aria2 owns

- **Velocity owns, and a new engine can reuse:** input validation, naming, categories, conflict policy, records and history, the schedule window, app-level retry classification, finalization checks, redaction (`redact.rs`), the browser bridge, the libtorrent engine, and the status vocabulary. `DownloadStatus` already has Queued, Connecting, Downloading, Paused, Verifying, Retrying, Stalled, Completed, Failed, Removed and the torrent states.
- **aria2 owns, and a new engine must replace:** the queue and concurrency limit, segment planning and assignment, connection reuse, per-download and global speed limits, resume state (the `.aria2` control file), the session file, checksum verification, Metalink, mirror selection (`--uri-selector=feedback`), low-speed detection (`lowest-speed-limit`), and HTTP/FTP protocol handling.
- **aria2-shaped code that would have to be generalized:** `EngineRouter`, `Aria2Status` across the manager, the `aria2::codes` exit-code mapping, aria2 option names inside `options_json`, `optimizer.rs` (built around aria2's `split` semantics), GID prefix routing, and session-file scrubbing.

### Known gaps in current behaviour (VERIFIED unless marked)

- **HTTP/1.1 only.** aria2 sends `HTTP/1.1` request lines (`vendor/aria2/src/HttpRequest.cc:167`) and contains no HTTP/2 code.
- **No validator-based resume.** The vendored source has no `ETag`/`If-Range` handling. A resume checks only `Content-Range` against the requested range and the entity length (`HttpResponse.cc:71`). A file changed on the server but kept the same size can be stitched from two versions without being detected. That is INFERRED from the source, NOT TESTED.
- **Windows TLS:** the WinTLS (Schannel) backend has no TLS 1.3 and ignores `ca-certificate` (`docs/ARIA2_INTEGRATION.md`).
- **SFTP:** not supported. Both vendored builds are configured `--without-libssh2`.
- **Changing connections restarts a running transfer**, because aria2 applies `split` by restarting it.
- **Credentials are lost on restart** by design: downloads that need a cookie or password fail after restart until they are re-added.
- **Integrity:** finalization checks size only. There is no per-chunk verification unless Metalink supplies chunk hashes.
- **aria2 limits:** at most 16 connections per server, and `min-split-size` is 1 MiB at minimum.

Added in revision 2 (VERIFIED):

- **Unknown engine values are read as aria2.** `database/downloads.rs:139-142` maps every `engine` string other than `libtorrent` to `EngineKind::Aria2`. A record written by a newer engine would be routed to aria2 by this code. Today this is prevented only because a database with a newer schema version is refused (`database/mod.rs:65`).
- **Release builds abort on panic.** The workspace `Cargo.toml` sets `panic = "abort"` and `lto = true` for release builds. A panic or a native crash in any in-process engine thread ends the whole app, including the UI and libtorrent.
- **Sidecars are an established pattern.** `src-tauri/tauri.conf.json:46` already ships `velocity-aria2c` and `velocity-native-host` as `externalBin`. Bundle `targets` is `"all"`, which includes AppImage on Linux. That matters for library bundling (section 8).

### Existing tests and benchmarks

- `tests/engine_integration.rs` has 36 real-aria2 tests (36/36 PASS on `chore/35-integration` f5cb7ad on Linux and native Windows, per the issue #35 evidence comment). They use a local HTTP/HTTPS server with ranges, throttling, failure injection and redirects, and a passive FTP server (`tests/common/ftp.rs`). They cover TLS certificate-purpose, host-name and expiry rejection, conflicts, queue and speed limits, and credential scrubbing. **These are engine-neutral behaviour tests that a new engine should pass unchanged.**
- `tests/torrent_integration.rs`: 38 tests with libtorrent, 31 without. They are not affected by an HTTP engine change.
- `scripts/bench/run-benchmarks.mjs` (329 lines) runs a Node HTTP/1.1 **plaintext** loopback server with per-connection throttling, latency, no-Range and connection-reset modes, using SHA-256 checks. CPU and RSS are sampled from `/proc`, so they are Linux-only. The latest saved run is `bench-2026-10-18T12-52-26-548Z` (3 runs per case). It has **no TLS, HTTP/2, 429/503, malformed-range, changed-object, FTP, SFTP or Windows scenarios**, and no fairness metric.
- The existing `reqwest 0.13` dependency is built **without TLS** (`default-features = false, features = ["json"]`). It is used only for loopback RPC.

Facts from the saved benchmark run `bench-2026-10-18T12-52-26-548Z` that matter for this revision (VERIFIED by reading the `.md` and `.json`):

- **Stock aria2 beat every Velocity profile on 40 × 1 MiB files**: 4.44 s against 7.52 to 7.62 s. The cause is the queue: stock runs 5 downloads at once, while Velocity's profiles allow 3 (peak 9 against 6 server connections).
- **Velocity Maximum opened 16 connections and made 16 GET requests against the server without Range support**, where stock aria2 made one. The extra bytes those requests transferred were not measured, because the harness does not count bytes sent.
- **Only 3 runs per case.** Times are medians of verified runs. The order is fixed (all runs of one runner, then the next).
- **The harness options are `--runs`, `--quick` and `--aria2` only.** Resource sampling reads `/proc`. The curl runner uses `sh -c`, and `getconf` is called for clock ticks. **The harness cannot measure resources on Windows** (`run-benchmarks.mjs:127-160, 191-200`).

## 2. Candidate comparison

**None of the candidates is a drop-in engine that is both permissively licensed and complete.** libcurl is the strongest *transfer library*: HTTP/1.1, HTTP/2, FTP and FTPS, SFTP, proxies, mature TLS on every platform, and a curl licence. Velocity must still write everything a download manager adds on top: segmentation, scheduling, resume state, Metalink, mirrors and integrity. The full engines that do this work (aria2, Wget2, Axel, Gopeed) are all GPL.

| Candidate | Kind | What it gives Velocity | What it lacks | Licence | Verdict |
| --- | --- | --- | --- | --- | --- |
| aria2 1.37.0+42 (vendored `9e72735`) | Full engine, separate process | Segmentation, queue, `.aria2` resume, Metalink v3/v4, mirrors with `feedback` selection, FTP, checksums, RPC | HTTP/2 and HTTP/3, ETag/If-Range validation, TLS 1.3 on Windows (WinTLS), SFTP in Velocity's builds, more than 16 connections per server, live re-planning without a restart; upstream last released in 2023 | GPL-2.0-or-later with OpenSSL exception | Baseline and rollback. Keep it until a replacement passes parity. |
| [libcurl](https://curl.se/) 8.23 (2026-10-14) | C transfer library | Multi interface (many transfers on one thread), connection reuse, HTTP/1.1, [HTTP/2 multiplexing by default](https://curl.se/libcurl/c/CURLMOPT_PIPELINING.html), [ranges for HTTP/FTP/SFTP/FILE](https://curl.se/libcurl/c/CURLOPT_RANGE.html), resume offsets, FTP/FTPS, SFTP (with libssh2), all proxy types, per-transfer rate caps, low-speed abort, HSTS, a [per-host connection cap that queues](https://curl.se/libcurl/c/CURLMOPT_MAX_HOST_CONNECTIONS.html) | Segment planning, global rate fairness, resume metadata, Metalink ([disabled in curl since 7.78.0](https://curl.se/docs/manpage.html)), mirror health, integrity, queue | [curl licence](https://curl.se/docs/copyright.html) (MIT-like; keep the notice) | **Recommended transfer layer.** |
| [`curl` crate](https://docs.rs/crate/curl/latest) 0.4.51 / `curl-sys` 0.4.91 (bundles libcurl 8.22.0) | Rust binding | Safe `Easy2`/`Multi` API, Schannel on Windows, OpenSSL or rustls options, optional static build | Its [bundled build](https://raw.githubusercontent.com/alexcrichton/curl-rust/main/curl-sys/build.rs) has **no SFTP (libssh2) and no HTTP/3**. It bundles exactly curl 8.22.0, which is affected by 21 advisories fixed in 8.23.0 (rev 2, VERIFIED). Its bundled Linux build uses \`select()\` (open issue #684). A segfault fix (#645) is unreleased. There is no async integration; Velocity would drive `Multi` on its own thread. | MIT | Usable only with libcurl 8.23.0 or later and the poll fix, through an upgraded release, a reviewed git pin, or Velocity's own libcurl build (section 8). SFTP still needs a separately built libcurl or a separate backend. |
| [reqwest](https://docs.rs/reqwest/latest/reqwest/) 0.13.5 + Tokio + hyper | Rust HTTP client | HTTP/1.1 and HTTP/2, rustls or native-tls, cookies, gzip/brotli/zstd, SOCKS5, async streaming; already in the tree | HTTP only. HTTP/3 is `reqwest_unstable`. No FTP or SFTP. No per-host connection queueing, rate limiting or low-speed detection. Range handling is manual. | MIT OR Apache-2.0 | An HTTP-only fallback. SFTP is now required, so it would need FTP ([suppaftp](https://docs.rs/crate/suppaftp/latest) 12.2.1, MIT/Apache) and SFTP (russh plus russh-sftp) backends. |
| [GNU Wget2](https://gitlab.com/gnuwget/wget2/-/raw/master/README.md) / libwget | Tool and library | HTTP/2, multi-threaded `--chunk-size`, Metalink RFC 5854/6249, HSTS | No FTP in Wget2. GnuTLS-centred. Weak Windows story. | Wget2 GPL-3.0+, libwget LGPL-3.0+ | Not recommended: copyleft, and the Windows build risk. |
| [Axel](https://github.com/axel-download-accelerator/axel) | CLI accelerator | Simple multi-connection HTTP/HTTPS/FTP/FTPS, mirror load-balancing | No library, no HTTP/2, no SFTP, no Metalink | GPL-2.0+ with OpenSSL exception | Design reference only. |
| [Gopeed](https://raw.githubusercontent.com/GopeedLab/gopeed/main/README.md) | Go engine and app | Its own multi-connection HTTP plus BitTorrent | Go runtime, a separate stack | GPL-3.0 | Not suitable. |

### SFTP backends (new in revision 2)

**Neither SFTP option is ready to approve on paper. Both need a prototype that proves strict host-key handling and resume against a local OpenSSH server.**

|  | libssh2 through libcurl | russh 0.64.1 + russh-sftp 3.0.1 (pure Rust) |
| --- | --- | --- |
| Licence | BSD-3-Clause (libssh2); Rust wrappers `ssh2`/`libssh2-sys` MIT OR Apache-2.0 (crates.io, VERIFIED) | Apache-2.0 for both crates (crates.io metadata, VERIFIED); its crypto dependencies are not yet enumerated (NOT TESTED) |
| Releases and activity | Last release **1.11.1, 2024-10-16**. Commits continue on `master` (latest 2026-10-21). `libssh2-sys` 0.3.3 bundles an **unreleased** master commit `4aded1bf` (2026-08-04), 948 commits after 1.11.1 (VERIFIED) | russh 0.64.1 (2026-10-05); russh-sftp 3.0.1 (2026-09-28), last commit 2026-10-01 (VERIFIED) |
| Host-key control | libcurl `CURLOPT_SSH_KNOWNHOSTS` and `CURLOPT_SSH_KEYFUNCTION`; `CURLOPT_SSH_HOST_PUBLIC_KEY_SHA256` for pinning. **CVE-2026-87865**: with a hashed `known_hosts`, libcurl 7.69.1 to 8.22.0 could report a *changed* key as *missing*, defeating trust-on-first-use (VERIFIED, [advisory](https://curl.se/docs/CVE-2026-87865.html)) | `client::Handler::check_server_key`. Its default **rejects all keys**, and the app must implement trust ([docs](https://docs.rs/russh/latest/russh/client/trait.Handler.html), VERIFIED) |
| Resume | `CURLOPT_RESUME_FROM_LARGE` / `CURLOPT_RANGE` list SFTP among affected protocols (VERIFIED) | `File` implements `AsyncSeek` + `AsyncRead`; `SeekFrom::Start` gives offset reads (VERIFIED) |
| Security record | Tracked in curl's and libssh2's advisories | [RUSTSEC-2026-0154](https://rustsec.org/advisories/RUSTSEC-2026-0154.html) (High, unbounded allocation in the agent client and server, fixed in 0.60.3 or later) (VERIFIED) |
| Build | Needs a custom libcurl, because `curl-sys`'s bundled build has no SSH | Cargo only; a second protocol stack beside libcurl |

Recommendation: prototype **both** against the same SFTP acceptance tests (section 5.4). Choose on host-key correctness, resume, throughput and closure. Do not choose on language or licence alone (INFERRED).

### Competitors (user-visible behaviour only)

- **Internet Download Manager** (proprietary, no public source). It [documents dynamic segmentation](https://www.internetdownloadmanager.com/features2.html): "When new connection becomes available IDM finds the largest segment to download and divide it in half." It also has connection reuse, HTTP/HTTPS/FTP/MMS, queues, a scheduler, proxy auth (Basic, NTLM, Kerberos) and an antivirus hook. Nothing beyond that page is inferred.
- **Free Download Manager 6** (proprietary). It [lists](https://www.freedownloadmanager.org/features.htm) segmented downloading, BitTorrent, traffic modes, a scheduler, type-based folders, media preview and resume. Older GPL releases of FDM 3.x/5.x exist, but they are not FDM 6. Their applicability is unverified, so they were not used.
- **Motrix** is an Electron front end for aria2 ([README](https://raw.githubusercontent.com/agalwood/Motrix/master/README.md), MIT). It is not an independent engine.
- **Persepolis** was an aria2 front end until 5.0.0. Its release notes say it then [dropped aria2 for its own `persepolis_lib`](https://videohelp.com/software/Persepolis-Download-Manager/version-history), citing aria2's 16-connection cap, missing SOCKS5 and RPC crashes. This is a secondary source; the upstream release notes were not opened.

### What matters for performance (INFERRED from documentation and design, NOT TESTED)

- **Per-connection throttling** is the main case where segmentation helps (Velocity's own benchmark showed about 7.6× with 8 connections). **With HTTP/2, libcurl multiplexes by default**, so eight ranges may share **one** TCP connection and gain nothing against a per-connection limit. A libcurl engine must decide whether to multiplex or open separate connections, per host, using measured results. aria2 avoids this question because it only speaks HTTP/1.1.
- **Loopback overhead:** curl streamed 965 MiB/s against aria2's 292 to 321 MiB/s in the saved benchmark. libcurl's copy path is lean. Velocity's write path (buffering, positional writes, checkpoints) will decide whether that advantage survives.
- **Threads:** one `Multi` loop on a dedicated thread handles hundreds of transfers. Disk writes should move to a bounded writer pool so a slow disk applies back-pressure (`CURL_WRITEFUNC_PAUSE`), rather than blocking the network loop.
- **Maintenance:** libcurl gets frequent releases and security advisories ([curl security](https://curl.se/docs/security.html)). Velocity must track them, as it already must for aria2, but it no longer owns the protocol code. libssh2's latest release, 1.11.1 (2024-10-16), is two years old, which makes SFTP the weakest-maintained dependency.

## 3. Feature-parity matrix

**Of 41 capabilities, libcurl directly supplies the protocol part of about half. Velocity must build the policy part of almost all of them.** Abbreviations: **Req** = required for replacement; **Adv** = advanced parity; **Comp** = competitive enhancement; **Opt** = optional. Confidence: **V** = verified in code, tests or official docs; **I** = inferred; **NT** = not tested. "Velocity today" cites the code or test that shows the behaviour.

*Revision 2 changes to this matrix:*

- SFTP is now **required** (per the brief), behind a backend prototype.
- The probe, changed-object, splitting and checksum rows were rewritten to match section 5.
- "Incremental hashing" no longer claims to avoid a verification pass.
- Numeric thresholds (4 MiB, ±20%, 2 s, 30 s, 64 MiB) are **starting hypotheses** to be tuned in stage 3. They are not requirements.
- Every "Velocity today" claim cites code or tests read at `e9d717a8`. libcurl claims cite the official option pages in section 11.

### A. Protocols and transfers

| Capability | Velocity today | aria2 | libcurl provides | Velocity must build | Extra dependency | Licence / security | Priority | Acceptance test | Conf. |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| HTTP/1.1 and HTTPS | Via aria2; `engine_integration` 36 tests | Yes | Yes | Mapping to records and statuses | TLS backend | Keep verification on; CVE-2026-8367 tests | Req | Run the existing `engine_integration` HTTP/HTTPS tests unchanged against the new engine | V |
| HTTP/2 | None | No (HTTP/1.1 only, `HttpRequest.cc:167`) | Yes, with nghttp2; multiplexes by default | Multiplex-or-separate-connections policy per host | nghttp2 (MIT) | ALPN; stream-count limits | Comp | A local h2 server (rustls test fixture) completes; the connection count matches the chosen policy | V/I |
| HTTP/3 | None | No | ngtcp2 backend stable; quiche experimental ([doc](https://curl.se/docs/http3.html)) | Fallback to TCP, Alt-Svc cache | ngtcp2 + nghttp3 (MIT), OpenSSL 3.5+ | UDP firewall issues; larger attack surface | Opt (defer) | Not until h1/h2 parity; a local QUIC server test | V |
| FTP (passive, EPSV) | Via aria2; `tests/common/ftp.rs` | Yes | Yes | Credential handling, size via SIZE | None | Cleartext: warn the user | Req | The existing FTP tests (credentials never stored, missing file fails) | V |
| FTPS (explicit/implicit) | Not offered | Not in builds | Yes | UI opt-in, scheme `ftps` | TLS | Verify certificates | Opt | A local FTPS fixture with valid and invalid certificates | I |
| SFTP | Refused (`validation.rs:75`) | Not in builds (`--without-libssh2`) | Yes, with libssh2 or libssh; **not in the curl-sys bundled build** | known\_hosts store, trust-on-first-use dialog, key and password handling | libssh2 (BSD-3) or russh (Apache-2.0) | Strict host-key checks; never accept an unknown key silently; libcurl 8.23.0 or later (CVE-2026-87865); SHA-256 key pin; no hashed known\_hosts | Req (rev 2; backend gated by prototype) | A local OpenSSH container: an unknown key is refused until accepted, a changed key always fails, a resume after a cut matches SHA-256 | V/I |
| Redirects | aria2 follows them | Yes | Yes (`FOLLOWLOCATION`, `REDIR_PROTOCOLS_STR`) | Limit to 10, scheme allowlist, no https→http downgrade by default, final-URL naming | None | Strip `Authorization` and cookies across origins | Req | Fixture: a cross-host redirect drops credentials; https→http is refused | I |
| Auth, cookies, headers, referrer, user agent | Held in memory; header denylist | Yes | Yes | Keep in-memory secrets; per-origin cookie scope | None | Never on disk or argv | Req | Credential tests plus a new "no secret in DB, logs or temp files" scan | V |
| Proxies (HTTP, HTTPS, SOCKS4/5) and proxy auth | Not configurable | HTTP proxy only | Yes, all types; system-proxy detection is partial | Proxy settings UI, PAC/system proxy policy | None | Proxy credentials in the OS keychain, or in memory only | Adv | A local HTTP and SOCKS5 proxy fixture; proxy credentials never logged | I |
| DNS | aria2 with c-ares | c-ares | Threaded resolver; c-ares or DoH optional | IPv4/IPv6 preference, DNS-failure classification | None | DoH is optional | Req | A DNS failure is classified as transient | I |
| Size, metadata and filename discovery | aria2 plus `validation.rs` naming | Yes | Headers via callback; `Content-Length`, `Content-Disposition` raw | Header parsing (RFC 6266), reuse of the existing naming rules | None | Treat names as untrusted | Req | The existing #46 Content-Disposition tests | V |
| Explicit capability detection | Version and features line from `--version` | Yes | `curl_version_info` (protocols, features) | Report capabilities per build and refuse unsupported schemes up front | None | — | Req | A unit test: a build without SFTP refuses `sftp://` with a specific message | I |
| Mirrors and alternate URLs | Not exposed | Yes, with `uri-selector` | No | Source pool, health scoring, failover | None | All sources must serve identical bytes, so verify | Comp | Two mirrors, one bad: completes from the good one, the bad one is demoted | I |
| Metalink v3/v4, Metalink/HTTP | Off by default (`enable_metalink`) | Yes | **No** (disabled since 7.78.0) | XML parser, source selection, chunk hashes, file selection | quick-xml (MIT) | XML bombs, path traversal in names | Adv | The RFC 5854 sample corpus plus malicious-name cases | V |
| BitTorrent v1, v2, hybrid, magnet | libtorrent 2.0.x (`velocity-lt`) | v1 only | No | Nothing: unchanged | libtorrent (BSD-3) | Unchanged | Req (keep) | The existing `torrent_integration` 38 tests stay green | V |

### B. Segmentation and acceleration

| Capability | Velocity today | aria2 | libcurl provides | Velocity must build | Extra dependency | Licence / security | Priority | Acceptance test | Conf. |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Range support detection | aria2 internal | `Accept-Ranges` plus a probe | `CURLOPT_RANGE`; servers may ignore it | Bounded first-segment probe (section 5.1); require 206 plus a matching `Content-Range`; `Accept-Ranges` alone proves nothing | None | — | Req | The no-Range server falls back to one connection without wasted bytes | V |
| Validating 206 / `Content-Range` | `validateResponse` | Checks the range and entity length | Raw headers | A strict validator (start, end, total, status) for every response | None | Prevents corruption | Req | A server that returns the wrong range or a changed total: the segment is rejected and the file is still correct | V |
| Changed-object detection | None (no ETag) | None | Headers; `If-Range` can be set | Strong ETag, or Last-Modified only if strong (RFC 9110 §8.8.2.2); `If-Range` on every range request; never append a 200; restart once, then fail (section 5.3) | None | Prevents stitching two versions | **Comp** (beats aria2) | A server swaps the file mid-download: restart or fail, never a mixed file | V |
| Content-Encoding safety | aria2 decodes gzip and deflate | Partial | `ACCEPT_ENCODING` | Never request encoding for ranged transfers; refuse to segment an encoded response | None | — | Req | A gzip-only server: single stream, correct bytes | I |
| Dynamic splitting | Static `split` and `min-split-size` | Fixed pieces; idle connections take unused pieces | No | Work-stealing with exclusive leases, lease ids and bounded write acceptance (section 5.2) | None | — | Req | Bytes are never overlapping or missing (a property test over random cut points) | I |
| Adaptive connection count | Optimizer, with a step-up that **restarts** | Restarts when `split` changes | No | Add or drop workers live, without restarting; ramp up while throughput rises; back off on 429, 503 or resets | None | Be polite to servers | Comp | A throttled fixture: reaches at least 95% of the best static plan's throughput; never exceeds the caps | I |
| Per-download, per-host and global budgets | Per-download cap 8, budget | 16 per server | `MAX_HOST_CONNECTIONS` (queues), `MAX_TOTAL_CONNECTIONS` | A scheduler that allocates the budget across downloads fairly | None | — | Req | 4 concurrent downloads get throughput within ±20% of equal | I |
| Small files | Single connection below `2×min-split` | Same | Connection reuse | Do not segment below a tuned minimum (hypothesis: 1 to 4 MiB); reuse connections across queued small files | None | — | Req | 40×1 MiB: at least as fast as stock aria2 (4.44 s in the saved run) | V |
| Overlap and gap safety | aria2 bitfield | Bitfield | No | Range-set accounting, positional writes (`pwrite`/`seek_write`) | None | — | Req | A fuzzed scheduler plus a SHA-256 check on every benchmark run | I |

### C to I. Manager, recovery, integrity, resilience, security, resources, licensing

| Capability | Velocity today | aria2 | libcurl provides | Velocity must build | Extra dependency | Licence / security | Priority | Acceptance test | Conf. |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Queue, order, priorities, concurrency limit | aria2 queue plus `change_position` | Yes | No | A Velocity-owned queue in core (it already owns the schedule) | None | — | Req | The existing queue-limit tests, plus reordering | V |
| Pause, resume, cancel, remove, retry | Via RPC | Yes | `curl_easy_pause`, remove handle | State machine; resume from a checkpoint | None | — | Req | The existing pause and resume tests, plus pause during a segment split | V |
| Add paused, schedule, live settings | `pause=true`, `schedule.rs` | Yes | — | Apply limits live with no restart | None | — | Req | Changing the speed limit mid-transfer takes effect in 2 s or less | I |
| Speed limits, per download and global | aria2 options | Yes | Per transfer (approximate) | A global token bucket redistributed across transfers | None | — | Req | A global 4 MiB/s limit holds within ±10% over 30 s | I |
| Duplicates, conflicts, history, progress, ETA | Velocity (#39, #46) | — | — | Reuse as-is | None | — | Req | Existing tests | V |
| Durable progress checkpoints | aria2 control file plus a 15 s session | `.aria2` bitfield | No | A `.vdm` sidecar or SQLite rows: completed ranges, validators, size, version; fsync before the record advances | None | No secrets in the checkpoint | Req | Kill -9 at random points over 100 trials: always resumes; bytes re-downloaded at most one checkpoint interval | I |
| Engine or app crash recovery | Reconcile, re-add, `ENGINE_LOST` | Session file | — | In-process: recovery runs at startup from the checkpoints | None | — | Req | Crash during a split and during finalization | I |
| Atomic completion | Size check only | Writes in place | — | Write to `name.vdmpart`, verify, fsync, rename without replacing (no-clobber) | None | Never overwrite without consent | Req | Crash before the rename leaves no completed-looking file | I |
| Checksums: whole file and per chunk | User checksum checked by aria2 | Yes, plus Metalink chunks | No | Whole-file hash in one sequential pass (a contiguous-prefix hash frontier plus a read-back); per-piece re-fetch only with trusted piece hashes (Metalink); section 5.3 | sha2 (already a dependency) | — | Req | A wrong checksum fails and the file is never marked complete; with Metalink piece hashes, only the corrupt piece is re-fetched | V |
| Filenames, Unicode, reserved names, traversal | `validation.rs` | Partial | — | Reuse | None | Untrusted input | Req | Existing validation tests | V |
| Disk full, permissions, links | aria2 error codes | Yes | — | Preallocation choice, ENOSPC classification, no-follow opens | fs4 (already a dependency) | Never follow links or reparse points | Req | A tmpfs quota fixture: a clear "disk full" error, and the partial file kept | I |
| Retry classes, backoff, jitter, `Retry-After` | `aria2::codes`, 60 s for 429/503 | Partial | Result codes and HTTP status | Classifier on `CURLcode` and status; honour `Retry-After` (capped); per-segment retries | None | — | Req | 429 with `Retry-After: 5`: waits 5 s or more, does not hammer | V |
| Stall detection | `lowest-speed-limit` | Yes | `LOW_SPEED_LIMIT`/`TIME` | A watchdog on bytes per window; no reset while bytes still flow | None | — | Req | A 20 KiB/s healthy server is never reset; a frozen socket is reset within 30 s | I |
| Diagnostics and redaction | `redact.rs` | — | Debug callback | Redact URLs, headers and keys in curl debug output | None | Never log `Authorization` | Req | A log scan test for secrets | V |
| TLS on Windows | Schannel without TLS 1.3 | WinTLS | Schannel (TLS 1.3 on supported Windows) or OpenSSL | Choose a backend | Schannel or OpenSSL | Revocation-check behaviour differs | Req | The certificate-purpose, host and expiry tests on real Windows | V/I |
| Memory and back-pressure | aria2 disk cache | Disk cache | `BUFFERSIZE`, write pause | A bounded write queue | None | — | Req | RSS at most 64 MiB with 4 × 16 connections | I |
| Licence closure | aria2 GPL sidecar | GPL | curl licence | Notices | See section 8 | No copyleft in the default build | Req | `generate-notices.mjs` reviewed | V |

## 4. Recommended design

**Recommendation (revised): a Velocity-owned transfer engine for direct downloads (HTTP/1.1, HTTP/2, FTP, FTPS and, once a backend is chosen, SFTP), using libcurl 8.23.0 or later as the transport. It sits behind a small, transport-neutral contract used only by direct downloads, so it can run in-process or as a supervised child process without changing the manager.** libtorrent keeps all torrent work. aria2 remains the legacy engine for its own records. Approval of the full engine waits for the prototype gates in section 7.

&#91;embedded content: proposed engine architecture · new parts highlighted\]

The manager and UI keep their policy. Only the engine layer gains a neutral contract and a new libcurl-based engine beside libtorrent and the legacy aria2 path.

### Why libcurl over reqwest, given the trade-offs

| Criterion | libcurl via the `curl` crate | reqwest + hyper + separate FTP/SFTP |
| --- | --- | --- |
| Protocol coverage from one stack | HTTP/1.1, HTTP/2, FTP, FTPS, SFTP, every proxy type, HSTS | HTTP/1.1 and HTTP/2 only; FTP and SFTP need two more young crates |
| Connection control a download manager needs | Per-host queueing cap, multiplex on/off, fresh-connect, low-speed abort, rate cap, pause/unpause | Pool settings only; caps, rate limits and stall detection are custom |
| Windows TLS | Schannel (system store, TLS 1.3 on supported Windows) or OpenSSL | rustls (needs `rustls-platform-verifier` for the system store) or native-tls |
| Safety | C library in-process: memory-safety bugs are curl's (tracked CVEs) | Pure Rust: memory safe |
| Build | C build via `curl-sys`. SFTP or HTTP/3 needs a custom libcurl build. | Cargo only |
| Fit with Tokio | A dedicated thread driving `Multi`, with channels | Native async |
| Licence | curl (MIT-like) | MIT/Apache |

libcurl wins on protocol breadth, the connection controls segmentation needs, and long server-compatibility history. reqwest wins on memory safety and build simplicity. **SFTP is now required, so reqwest would need two more protocol stacks (FTP and SFTP). It stays only as an HTTP fallback, measured in stage 3 if the libcurl prototype misses an HTTP gate.** A permanent hybrid (reqwest for HTTP, libcurl for FTP) is **not** recommended: two HTTP-capable stacks double the TLS, proxy and test matrix, with no measured gain (INFERRED).

### 4.2 The smallest coherent seam (revised)

Revision 1 proposed a general `Engine` trait for every engine. With about 55 call sites and torrent-specific behaviour throughout, that is a framework rewrite before any evidence exists. Revision 2 narrows it.

- **A `DirectTransfer` trait for direct downloads only.** It offers `add(TransferSpec)`, `pause`, `resume`, `cancel`, `remove`, `set_limits`, `set_connections` and `snapshot()`, plus an event stream. It returns Velocity's own `DownloadStatus` and a neutral `TransferError { class, http_status, message }`. It never returns `Aria2Status`.
- **Two implementations at first.** `Aria2Direct` wraps today's router calls for HTTP/FTP/Metalink records and moves the existing `aria2::mapping` into the adapter. `VelocityDirect` is the new engine. The manager's direct-download paths call the trait. **Torrent paths are untouched:** `reannounce`, `trackers`, `get_peers`, file priorities, seeding and magnet metadata stay on `EngineRouter` and `LtEngine`.
- **Capabilities, not pretence.** `capabilities()` reports what each engine really does: protocols, live connection changes (aria2: by restart), validator-checked resume, Metalink, maximum connections per host. The UI can then explain differences truthfully, as #44 already does for connection limits.
- **Routing by the record's `engine` column**, with a new value `velocity`, never by GID prefix. An engine missing from the build shows "needs the X engine" and keeps the files. The current parser, which maps unknown values to aria2, is fixed first (section 6).
- **Crates.** `crates/velocity-curl` is the only crate that touches libcurl (FFI, the `Multi` loop). A new `crates/velocity-transfer` holds the engine logic: probe, leases, planner, checkpoints, integrity and the stdio protocol. It is pure Rust and testable with a simulated transport, and it builds both as a library and as an optional `velocity-transfer` sidecar binary. `velocity-core` keeps policy, persistence and the adapters. Placing engine logic outside `velocity-core` is an architecture decision for the PO (section 10), as `velocity-lt` already was.
- **Unchanged:** validation, naming, categories, conflict policy, the browser bridge, the UI contract (`COMMAND_API_VERSION`) and libtorrent.

### 4.3 In-process or separate process (new in revision 2)

**Recommendation: build the contract so both are possible. Make a supervised child process (`velocity-transfer`, stdio framing) the production target, unless the stage 3 prototype shows a material cost.** Revision 1 chose in-process largely because libtorrent works that way. Revision 2 compares both against Velocity's actual boundaries.

| Concern | In-process | Child process over stdio |
| --- | --- | --- |
| Failure impact | Release builds use `panic = "abort"`. A Rust panic in the engine, or a native fault in libcurl, the TLS library, nghttp2 or libssh2, ends the whole app: the UI and the libtorrent session too (VERIFIED config, INFERRED effect) | Only the engine dies. The existing supervisor pattern (backoff, at most five restarts in five minutes) restarts it, and transfers resume from durable checkpoints. This is the same path as an app crash, so it is tested anyway (INFERRED) |
| Memory-corruption blast radius | Engine bugs can read the app's memory, IPC token and libtorrent state | Limited to the engine process. **It is not a sandbox:** same user, same file access. OS sandboxing (AppContainer, seccomp, App Sandbox) is DEFERRED |
| IPC and secrets | None needed | Inherited stdin/stdout pipes: **no listening socket, no port, no secret.** Only the parent holds the handles. Length-prefixed, versioned messages are validated on both sides, and engine messages are treated as untrusted. Credentials cross the pipe in memory only, never on argv or env. This is simpler than aria2's loopback port plus secret |
| Overhead | None | Control and progress messages only (at most about 10 per second per download). File data never crosses the pipe, because the engine writes files itself. One more process: aria2's was 18 to 34 MiB RSS in the saved benchmark. Cost NOT TESTED |
| Tokio and UI responsiveness | The `Multi` loop needs its own OS thread. Callbacks must never block or `.await`; disk writes go to a bounded pool | Same inside the child; the app's runtime is never touched |
| Cancellation and shutdown | Remove easy handles only on the `Multi` thread. The `curl` crate's drop-order segfault (#645) shows this is subtle | A graceful stop message, then a kill after a timeout. The same sequence aria2 uses today |
| Packaging | Nothing extra | A third `externalBin` (the pattern exists): Windows signing, macOS notarization, AV false-positive checks |
| Diagnostics | In the app log | stderr goes to the existing redacted ring buffer |

The prototype must measure, for the same scenarios: throughput and CPU difference between the two modes, event latency, RSS, crash-to-resume time after `kill -9` of the child, and shutdown time. If the child process costs no more than 2% throughput and adds at most 50 ms of event latency (both hypotheses), it should be the default.

### State model

The engine reports `Queued → Connecting → Downloading ⇄ Retrying(t) → Verifying → Completed`, with `Paused`, `Stalled` (no bytes for at least 30 s, not yet failed), `Failed(class)` and `Cancelled`. These are the existing `DownloadStatus` values, so the UI needs no new states.

## 5. Transfer correctness design (new in revision 2)

**Three rules carry this section. A probe never costs a second full transfer. A byte is accepted only from the one live lease that owns it. A file is marked Completed only after its coverage, its length and, when a trusted hash exists, its whole-file hash are proven.** The HTTP rules are VERIFIED against [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110). The design itself is NOT TESTED.

### 5.1 Range discovery (corrects R2)

Revision 1 opened with `Range: bytes=0-`. A server may ignore Range (RFC 9110 §14.2: "A server MAY ignore the Range header field") and send the whole file as `200`. The revised probe is the **first real segment**, so its bytes are never wasted:

1. **No HEAD.** HEAD can differ from GET: some CDNs, signed and one-time URLs refuse it, and it costs a round trip without providing body evidence. It is never used as proof of range support. `Accept-Ranges: bytes` is only a hint (RFC 9110 §14.3 says it "encourag\[es\]" ranges); it proves nothing.
2. **Request** `GET` with `Range: bytes=0-(S−1)`, where S is the initial segment size (hypothesis: 4 MiB). Send no `Accept-Encoding`; libcurl sends none unless `CURLOPT_ACCEPT_ENCODING` is set (INFERRED from the libcurl docs). Redirects are followed by libcurl within the protocol allowlist (section 8). Validators are taken from the **final** response only.
3. **Classify the final response in the header callback,** before any body byte is written. libcurl calls the header callback for every response, and returning a short count or `CURL_WRITEFUNC_ERROR` aborts with `CURLE_WRITE_ERROR` ([docs](https://curl.se/libcurl/c/CURLOPT_HEADERFUNCTION.html), VERIFIED).

| Final response | Meaning | Action |
| --- | --- | --- |
| `206`, one part, `Content-Range: bytes 0-b/T`, b ≤ S−1, numeric T, identity encoding | Ranges work | Keep the bytes as lease \[0, b+1). Record T and validators. A server may send *fewer* bytes than asked (RFC 9110 §14.2); the rest stays unassigned. State: `RANGES_OK`. |
| `206` with `bytes 0-b/*` (unknown length) | Partial support, no total | Keep \[0, b+1). No parallelism. Continue sequentially. State: `SEQUENTIAL_RANGES`. |
| `206` `multipart/byteranges` | A single range was asked for, so this is a protocol anomaly | Abort before writing. Retry once without Range, as a single stream. |
| `200` (Range ignored) | No range support for this request | **Keep this response as the single stream.** It started at byte 0, so nothing is wasted and no second transfer is made. Length is `Content-Length`, or unknown if chunked. State: `SINGLE_STREAM`. |
| `416` with `bytes */0` | Empty file | Complete with zero bytes after the normal finalization. |
| `401` / `407` | Credentials are needed | Fail with an auth error. No automatic retries. |
| `429` / `503` | Throttled | Back off; honour `Retry-After` (RFC 9110 §10.2.3), capped (hypothesis: 10 min). |
| Other `4xx`/`5xx`, malformed `Content-Range`, `Content-Length` ≠ b+1 on a `206` | Error or dishonest server | Abort before writing. Classify; transient errors retry with backoff. |

4. **Later range requests** send `Range: bytes=a-b` plus `If-Range` (section 5.3). A `200` for a request with a non-zero start is **aborted in the header callback, so its body is never written.** Only bytes already in socket buffers are lost; they are counted as wasted. Then the object is treated as changed, or ranges as lost (section 5.3).
5. **Accounting.** Every received byte is either accepted into coverage or counted in `wasted_bytes`. Progress shows accepted bytes only. The benchmark asserts that wasted bytes stay small (section 7).
6. **Chunked and short bodies.** For a `206`, `Content-Range` governs and `Transfer-Encoding: chunked` is allowed. If the body ends before b, the remainder returns to unassigned and the segment is retried. If it runs past b, the excess is dropped and counted, and the server is marked suspect.
7. **Inconsistent servers.** If two `206` responses for the same object report different T, validators or encodings, the download goes to the "changed" state (section 5.3). Two consecutive anomalies on one host fall back to `SINGLE_STREAM` for that download.

**State transitions:** `PROBING → RANGES_OK | SEQUENTIAL_RANGES | SINGLE_STREAM | FAILED`. From there, `RANGES_OK → CHANGED | RANGES_LOST`. `RANGES_LOST` continues as one stream from the first uncovered byte with `If-Range`. If that also returns `200`, the download restarts from 0 as a single stream, writing only into gaps, and only when the strong validator is unchanged. Otherwise it is `CHANGED`.

### 5.2 Range scheduler with exclusive leases (corrects R4)

Revision 1 split an in-flight range without saying how the original request was stopped. The revised design:

- **One owner.** All range state for a download is mutated by one owner: the `Multi` thread, or an actor it calls synchronously. No lock is held across I/O.
- **Coverage C:** disjoint byte intervals that were written to disk by their owning lease. **Durable coverage D ⊆ C:** intervals whose data was fsynced and then recorded in a checkpoint. D is the only coverage trusted after a crash.
- **Lease:** `{id (never reused), start s, cursor c, end e (exclusive), easy-handle token}`. The interval \[s, c) is already in C; \[c, e) is owned exclusively by this lease. Lease states:
  - `Assigned`: the request was created; no response yet.
  - `InFlight`: the `206` was validated.
  - `Completed`: c = e.
  - `Failed`: \[c, e) returns to unassigned.
  - `Revoked`: cancelled; \[c, e) returns to unassigned.
- **Write acceptance.** The write callback carries the lease id. Bytes \[p, p+n) are accepted only if the lease is live, p = c, and p+n ≤ e. A write that crosses e is accepted up to e; the callback then returns `CURL_WRITEFUNC_ERROR`, and the stop is recorded as `Completed`, not as an error. Bytes for a revoked or unknown lease id are dropped and counted as wasted.
- **Split (work-stealing).** When a worker is free and no unassigned interval is at least `min_split` long:
  1. Pick the live lease with the largest remainder e − c, which must be at least 2 × `min_split`.
  2. Set m = c + (e − c)/2, rounded to 64 KiB.
  3. **Shrink the old lease** to e := m and create a new lease \[m, e\_old).

  Because acceptance is checked against the *current* e on the owning thread, the old transfer can never write past m. With HTTP/1.1 it is stopped when it reaches m, which closes that connection. With HTTP/2, only the stream is reset. The cost of these stops is measured as wasted bytes and new connections. Workers request bounded segments (hypothesis: at most 32 MiB), which keeps splits rare.
- **Retries.** A failed lease's remainder becomes unassigned and is re-leased under a **new id**. A late callback from the old handle carries the old id and is rejected. libcurl does not call back after `curl_multi_remove_handle` returns, so this check guards against bugs. Duplicate or overlapping responses are impossible to accept, by the rule above.
- **Disk writes.** Accepted bytes go to a bounded writer queue. They enter C only when the positional write (`pwrite` or `seek_write`) returns. When the queue is full, the transfer pauses (`CURL_WRITEFUNC_PAUSE`) until there is room.
- **Checkpoint.** Every N seconds or M MiB (hypothesis: 2 s or 16 MiB), and at segment ends:
  1. fsync the `.vdmpart` file.
  2. Write the checkpoint atomically (write a temporary file, then rename; or one SQLite transaction). It holds a format version, URL fingerprint, T, validator, encoding and D.
- **Crash recovery.** Load D. Everything else is unassigned. Bytes on disk outside D are untrusted and will be overwritten. Leases are never persisted.
- **Windows note:** writing far past the end of a non-sparse NTFS file makes the OS zero-fill the gap synchronously. Preallocation (as aria2's `prealloc` default does) or sparse files must be chosen and measured (INFERRED, NOT TESTED).

**Invariants.** Each is checked after every simulated event:

- **I1 Partition:** \[0, T) = C ⊎ (⊎ over live leases of \[c, e)) ⊎ U, all disjoint.
- **I2 Exclusivity:** no two live leases overlap, and no lease's \[c, e) overlaps C.
- **I3 Acceptance:** a byte at offset p is written only by the live lease whose c = p < e.
- **I4 Monotonic coverage:** C only grows within a validator epoch. A validator change starts a new epoch with an empty C.
- **I5 Durability:** every byte in D was fsynced before D was persisted.
- **I6 Completion:** C = \[0, T) and no live lease, before verification starts.
- **I7 Conservation:** bytes received = growth of C + wasted bytes.

**Tests.**

- *Without a network:* a deterministic simulated transport with random chunk sizes, delays, splits, cancellations, retries, duplicate and late callbacks, `200` substitutions, and simulated crashes (runtime state dropped, D kept). Property tests assert I1 to I7 and byte equality with the source. A property-testing crate (for example `proptest`, MIT/Apache-2.0) is a new dev-dependency and needs approval; a seeded hand-written generator is the fallback.
- *Integration tests needed:* the real libcurl abort-at-boundary behaviour, HTTP/2 stream resets, real fsync and `kill -9`, Windows file semantics, and back-pressure under a slow disk.

### 5.3 Changed-object detection and integrity (corrects R5)

- **Strong vs weak.** An ETag prefixed `W/` is weak. `If-Range` uses the *strong* comparison, and "a client MUST NOT generate an If-Range header field containing an entity tag that is marked as weak" (RFC 9110 §13.1.5, §8.8.3.2, VERIFIED).
- **Last-Modified** may be sent in `If-Range` only when there is no entity tag *and* the date is strong. For a client, that means the response's `Date` is at least one second after `Last-Modified`, from the same clock (§8.8.2.2, VERIFIED). Velocity may require a larger margin; that is a hypothesis to test.
- **Validator choice:** use a strong ETag; otherwise a strong Last-Modified; otherwise **none**. With no validator, parallel segments within one session still check T on every response (no regression from aria2's behaviour today). After a pause or crash the download resumes with the status flag "not validated" shown in details. Whether to be stricter (restart from 0, or require a checksum) is a PO decision (section 10).
- **Server evaluation:** a false `If-Range` makes the server ignore Range and send `200` with the new representation (§13.1.5). Therefore **a `200` is never appended at a non-zero offset.** It is aborted before writing (section 5.1).
- **Per-response checks on every `206`:**
  - `Content-Range` start equals the requested start, its end is at most the requested end, and its total equals T.
  - The ETag, if present, equals the recorded one under strong comparison.
  - `Content-Encoding` matches the probe's.
  - If the probe had an ETag and this response omits it, treat the object as changed (conservative).
- **Changed object.** Stop all leases and discard C for the old epoch. Old bytes are never mixed with new ones: the `.vdmpart` is truncated, or moved aside if the user chose to keep partials. Restart from 0 once, with the new validator. A second change fails with "The file changed on the server during download" (retryable).
- **Redirects.** Validators and T belong to the representation, not the URL. A later request that lands on another final URL is accepted only with the same T and strong validator; otherwise the object has changed. An expired signed URL (`403`) is classified as "link expired", because it needs a fresh link from the browser.
- **Content-Encoding.** Ranges address the bytes of the selected representation. Velocity never asks for an encoding. If the server applies one anyway, Velocity stores the representation bytes exactly as sent, never decoding. Ranges are allowed only while every response has the same encoding. Diagnostics note it. This differs from aria2, which decodes gzip/deflate (`HttpResponse.cc:215`). It is a deliberate choice to test with a `.tar.gz` served with `Content-Encoding: gzip` (INFERRED).
- **Hashes do not compose.** The SHA-256 values of separate segments cannot be combined into the file's SHA-256. Revision 1's "no extra verification pass" was wrong. Instead:
  - A *hash frontier* hashes the contiguous prefix from byte 0 as it becomes covered, reading back from disk when needed. Whatever is left is read back at completion.
  - Worst case is one full sequential read, for example when the first segment finishes last.
  - Hasher state is not persisted, so after a crash the hash starts again from byte 0.
- **Trusted per-piece hashes** (Metalink `<pieces>`) verify each piece independently when it is fully covered. A mismatch removes that piece from C and re-fetches it, preferring another mirror. Hashes Velocity computes itself detect only disk errors, never wrong server content.
- **Completion gate.** All of the following, in order:
  1. C = \[0, T).
  2. The file length is T (single stream: `Content-Length`, or a clean end for chunked, flagged "size not confirmed by server").
  3. The whole-file hash matches, when one is available (user checksum, Metalink, or a `Repr-Digest` header, which is DEFERRED).
  4. fsync the file.
  5. Rename `.vdmpart` to the final name **without replacing** an existing file (Windows `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`; Linux `renameat2(RENAME_NOREPLACE)`; macOS `renamex_np(RENAME_EXCL)`).
  6. fsync the directory (Unix).

  With no hash available, the status says "Size verified", not "Verified".
- **Wrong checksum supplied:** fail with `INTEGRITY_CHECK_FAILED`. The file is not marked complete, and the partial is kept for inspection. A retry re-downloads once.

### 5.4 Acceptance tests for sections 5.1 to 5.3

Each row is a local fixture behaviour. Every test checks the final SHA-256, the status shown and the wasted bytes.

| Server behaviour | Expected result |
| --- | --- |
| Ignores Range (`200`) | One stream from the probe; zero extra GETs; correct file |
| `200` for a non-zero range | No body byte written; treated as changed or ranges lost; never a mixed file |
| Content changes mid-download with a new strong ETag | One restart; final file equals the new content |
| Content changes, ETag kept (dishonest) | Caught only when a checksum is supplied; otherwise documented as undetectable |
| ETag dropped mid-download | Treated as changed |
| Weak ETag only | No `If-Range` with it; Last-Modified used only if strong; otherwise "not validated" |
| Malformed `Content-Range` (wrong start, wrong total, `*/T`, garbage) | Segment rejected, retried, file correct |
| Fewer bytes than requested in a `206` | Remainder re-leased, file correct |
| `Content-Encoding` changes between responses | Ranges stop; restart as a single stream |
| Wrong user checksum | Failed, not completed; partial kept |
| Metalink piece hash wrong for one piece | Only that piece re-fetched |
| SFTP: unknown host key / changed key / known key | Refused until confirmed / always refused / accepted; resume after a cut matches SHA-256 |
| `kill -9` at random points, 100 trials | Always resumes; re-downloaded bytes are at most one checkpoint interval plus in-flight data |

### 5.5 Performance algorithm: starting hypotheses to tune

All numbers below are **hypotheses** for stage 3 experiments, not requirements.

| Knob | Starting value | How to tune |
| --- | --- | --- |
| Initial connections | 1, the probe | — |
| Ramp-up | Add 1 connection per evaluation window of max(2 s, 4 × RTT) while the aggregate gain is at least 10% | Sweep the gain threshold over 5, 10 and 20%, on servers that throttle per connection and per IP |
| Per-IP or per-account throttling | Stop and step back after 2 windows below 2% gain | The per-IP fixture must stop below the cap |
| Minimum segment and split size | 4 MiB / 1 MiB | Sweep 256 KiB, 1, 4 and 16 MiB on small, large and mixed sets |
| Per-host cap | 8 (as today), at most 16 | `CURLMOPT_MAX_HOST_CONNECTIONS` queues the excess (VERIFIED doc) |
| HTTP/2 | Multiplex first; switch the host to separate connections if added streams don't help | The per-TCP-throttle h2 fixture must pick separate connections; the per-stream fixture must keep multiplexing |
| Backoff | Transient errors: exponential with full jitter, 1 s base, 60 s cap. `429`/`503`: halve the host cap and honour `Retry-After` | The 429 fixture: no request before `Retry-After` elapses |
| Fairness | Max-min share of the global connection budget; a global token bucket for rate | Jain's index at least 0.9 in the concurrent scenarios |

**What libcurl controls directly:** the per-host and total connection caps (queued), multiplexing, fresh connections, timeouts, low-speed abort, per-transfer receive rate, and pause/unpause (VERIFIED from the option pages). **What Velocity owns:** probe classification, leases, splitting, ramp-up, fairness, the global rate, checkpoints, integrity, mirror choice and retries (INFERRED).

## 6. Migration and compatibility

**Recommendation: do not migrate unfinished aria2 transfers. Every existing record keeps `engine='aria2'` and finishes on aria2. Only new downloads go to the new engine.** This is the rule migration 0003 already applies to torrents, and it is the only option that cannot corrupt a partial file.

### What depends on aria2-specific state

| State | Engine-neutral? | Can it move safely? | Evidence |
| --- | --- | --- | --- |
| Completed and history records | Yes | Nothing to move | `RecordState::Completed`; files are final |
| Settings, categories, profiles | Mostly | Yes. Profiles become caps for the new engine; `options_json` keeps aria2 fields, and new fields are additive | `models.rs`, `optimizer.rs` |
| Unfinished HTTP/FTP partial files | No | **No, not reliably** | The `.aria2` control file holds a per-piece bitfield and in-flight pieces (`DefaultBtProgressInfoFile.cc`). It is written every 30 s (`auto-save-interval`). Whether data in `disk-cache` is always flushed before the bitfield claims a piece is NOT TESTED. **No ETag or Last-Modified is stored**, so a changed server file cannot be detected. |
| `aria2.session` (GIDs, URIs, options) | No | No | aria2-specific text format, scrubbed of credentials |
| Credentials and cookies | Never stored | — | Already lost on restart by design |
| Failed or retry-waiting records with data on disk | No | No | Same partial-file problem |
| Failed or retry-waiting records with **no bytes on disk** | Yes | Possible, but not recommended at first | The precedent says a record never changes engine |

### Options considered

1. **Keep legacy records on aria2 until they finish (recommended).** No data risk. The cost is that aria2 must stay bundled while any legacy record exists. A build without aria2 shows "needs the legacy engine" and keeps the files, mirroring `LIBTORRENT_MISSING`.
2. **Import the `.aria2` bitfield.** Technically parseable, but its flush ordering is unproven and there is no validator, so an imported resume could silently produce a mixed file. A full-file checksum check is impossible without a known hash. **Rejected**, unless a later prototype proves flush ordering and the user supplies a checksum.
3. **"Restart on the new engine" as an explicit user action.** It starts a fresh `.vdmpart` and keeps the old partial file until the new download is verified. Safe, but it costs bandwidth. It could be offered later for stuck legacy downloads.

### Migration mechanics (corrected in revision 2)

- **Fix the engine parser first (stage 1).** Today `database/downloads.rs:139-142` reads any unknown `engine` value as aria2 (VERIFIED). Revision 1 said old builds would "ignore the new values". In fact they would route a `velocity` record to aria2, which would start writing the final file name from scratch. Stage 1 adds `EngineKind::Unavailable(raw)`, which never resubmits and shows "needs the X engine". This ships **before** any build can write a new engine value.
- **Migration `0004` is additive and bumps the schema.** It allows `engine='velocity'`, and adds nullable validator columns (`etag`, `last_modified`, `validator_kind`) and a checkpoint reference. The existing `VACUUM INTO` backup runs first. Older builds then **refuse the database unmodified** (`database/mod.rs:65`, VERIFIED), rather than mis-route records. The consequence: downgrading below the release that adds 0004 needs the backup. The release notes must say so.
- **No record changes engine automatically.** Routing uses the record's `engine` column only.

### What can be handled differently, and what cannot

| Record state | Revision 2 handling | Why |
| --- | --- | --- |
| Completed and history | Engine-neutral. Nothing to move | The file is final and verified by the old rules |
| Settings and profiles | Profiles become *caps* for the new engine. aria2 fields in `options_json` are kept, and new fields are additive | `optimizer.rs` semantics are aria2-specific |
| Active or paused with data | **Stays on aria2 until it finishes or the user removes it** | The `.aria2` bitfield's flush ordering is NOT TESTED, and no validator is stored |
| Failed or retry-waiting **with zero bytes**, and no output file or `.aria2` file on disk | Stays on aria2 by default. Later, the explicit action "Restart with the new engine" may be offered | Technically safe once both files are proven absent (an `lstat`, never following links), but the rule "a record never changes engine" is simpler to reason about. Moving them is optional and needs its own tests |
| Torrents | Unaffected (libtorrent or aria2 as today) | — |

### Rollback with mixed engines

| Situation | Behaviour |
| --- | --- |
| The user switches "new engine for new downloads" off | New direct downloads go to aria2. Existing `velocity` records stay on the new engine and keep their checkpoints |
| A build without the new engine (or with it disabled at build time) opens the database | `velocity` records show "needs the Velocity transfer engine". Their files and checkpoints are kept. **Never** resubmitted to aria2 |
| A build without aria2 (after a future removal) | aria2 records show "needs the legacy engine"; files kept |
| The app is downgraded below the 0004 release | The database is refused unmodified; the user restores the `bak-v3` backup. Documented |
| The engine crashes repeatedly (sidecar model) | After the restart cap, its records show "Engine unavailable" with a manual restart, as aria2 does today |

User-facing plan: a one-time notice after the upgrade says that new downloads use the new engine and that downloads already in progress finish on the previous engine. Diagnostics list the number of legacy records per engine.

### Removing aria2 later

This is a separate owner decision. It should come only after the new engine has been the default for at least one release, and only when telemetry-free evidence exists. The app can count local legacy records at startup and show them in diagnostics, so the user knows nothing is left. Removing aria2 ends the GPL sidecar obligations only for later releases; obligations already incurred continue (section 8.3).

## 7. Benchmark and test plan

**Revised gates (R3): the new engine must be *no worse* than the best valid aria2 configuration in every scenario, on every guardrail metric. Superiority is claimed only in scenarios where a mechanism predicts it. A single corrupted file is an absolute blocker.** Nothing here has been run; all results are NOT TESTED.

### Why the revision-1 gates were unsound

- **They demanded a gain the physics rules out.** Against a server throttling each connection to 4 MiB/s, throughput is about N × 4 MiB/s for any engine using N connections (saved run: 16 connections gave 55.3 MiB/s). At *equal* N, a 10% gain is impossible, so that scenario can only show non-inferiority.
- **They compared with a weak baseline.** Velocity's profiles lose to stock aria2 on 40 small files (7.6 s against 4.44 s, because of queue size). Beating the profiles proves nothing. The reference must be the best valid aria2 configuration per scenario.
- **"100% correct" over a few runs is a weak statement.** With zero failures in n runs, the 95% upper bound on the failure rate is about 3/n (the rule of three): about 10% for n = 30. Silent corruption must therefore be a hard rule, not a statistic, and low failure *rates* need a soak test.

### Contenders

| Id | Contender | How it is configured |
| --- | --- | --- |
| A | aria2 stock | Defaults (1 connection, 5 concurrent) |
| B | aria2 with Velocity profiles | Exactly as shipped (`examples/profile_options.rs`, already in the harness) |
| C | **Best valid aria2** | Per scenario, the fastest configuration from a grid: `split` and `max-connection-per-server` in {1, 2, 4, 8, 16}, `min-split-size` in {1M, 4M, 8M, 20M}, `max-concurrent-downloads` in {3, 5, 8} (multi-file only), `file-allocation` in {prealloc, none}. "Valid" means within the same connection cap and concurrency the new engine uses. Selected on a **separate tuning run**, then measured on fresh runs, to avoid picking a lucky winner |
| D | New engine, static | N fixed to C's connection count |
| E | New engine, adaptive | Same caps as C, with ramp-up on |
| F | reqwest prototype | Only if the libcurl prototype misses an HTTP gate |
| — | curl CLI | A single-connection reference only, never a segmented comparison |

Kept identical across contenders: the machine, the server process, the files (seeded random bytes), the caps, the concurrency, the target disk, a fresh directory per run, a SHA-256 check per run, and the TLS configuration. **Loopback results gate the prototype. Real-network results are reported separately and never pooled.**

### Scenarios

The existing ones are kept. Rows marked + are new.

| Scenario | Mechanism under test | Claim allowed |
| --- | --- | --- |
| Per-connection throttle, 128 MiB | Segmentation at equal N | Non-inferiority (D, E against C) |
| + Per-IP or per-account throttle | Adaptive must stop adding connections that do not help | Non-inferiority, and E uses fewer connections than C |
| Unthrottled loopback, plus TLS and h2 variants (+) | Engine and TLS overhead | Non-inferiority; CPU guardrail |
| + h2 server throttling per TCP connection, and per stream | Multiplex policy | E must pick the right mode |
| 150 ms latency | Ramp-up under RTT | Non-inferiority |
| No Range support | Waste | E: one GET and wasted bytes ≈ 0 (Velocity Maximum made 16 requests in the saved run) |
| + Malformed `Content-Range`, + changed object, + `200` for a non-zero range, + unrequested gzip | Correctness (section 5.4) | Pass/fail only |
| Interrupted (resets) | Per-segment retry | Superiority allowed (recovery mechanism) |
| + `429` with `Retry-After`, + `503` | Politeness | Pass/fail (no request before `Retry-After` elapses) plus time |
| + Kill and resume, 20 kills | Checkpointing | Recovery time and re-downloaded bytes compared with C |
| 4 × 32 MiB concurrent | Fairness | Jain's index; non-inferiority |
| 40 × 1 MiB | Connection reuse, queue overhead | Superiority allowed (reuse mechanism), against **C** (stock aria2 4.44 s is the bar to beat, not the profiles) |
| + 1 × 1 GiB with 20 small files | Mixed workload, starvation | Non-inferiority; small-file latency |
| + FTP 64 MiB, + SFTP 64 MiB (if approved) | Protocol parity | Non-inferiority against aria2 FTP; SFTP has no aria2 baseline, so absolute numbers plus curl or OpenSSH `sftp` as a reference |
| + Slow but healthy, 20 KiB/s | Stall detection | Zero stall resets |

### Gates

1. **Hard rules** (one violation fails the scenario, with no statistics):
   - Any output that fails SHA-256 while the engine reports success.
   - Any file shown as Completed that is not the source.
   - More connections than the cap.
   - A request sent before `Retry-After` elapses.
   - A secret found in a log or a file.
2. **Reliability:** the new engine's count of non-completions must not exceed C's in any scenario. A failure-rate claim (for example "under 1%") needs a soak test of at least 300 runs per scenario in stage 9; until then, report the observed counts and the rule-of-three bound.
3. **Non-inferiority (time):**
   - R = median time(new) / median time(C).
   - The 95% CI comes from a paired-block bootstrap over repetition blocks.
   - Pass when the upper bound is at most 1.05. The 5% margin is a hypothesis to confirm from pilot variance.
4. **Superiority (only for the allowed scenarios above):** an upper bound of R at most 0.90, with Holm–Bonferroni correction across all superiority claims made.
5. **Guardrails against C, all required** (thresholds are hypotheses):
   - CPU-seconds per GiB at most 1.25×.
   - Peak RSS at most 1.5×, or 64 MiB, whichever is larger.
   - Wasted bytes at most max(1 MiB, 0.5% of the file) and no more than C's.
   - GET requests at most C's + 10%.
   - Bytes written to disk at most 1.05 × the file size, plus checkpoints.
   - Jain's index at least 0.9.
   - Recovery time after a kill no worse than C's.

A speed gain cannot hide extra failures, waste, requests or resource use, because each is a separate required gate.

### Trials and statistics

- **Pilot:** 10 blocks per scenario to estimate the coefficient of variation. Then n = max(15, enough for a CI half-width of 2% or less) blocks. A *block* is one run of every contender, in **random order**. The current harness runs all of one runner and then the next, which confounds order with time.
- **One discarded warm-up block** per scenario.
- **Outliers are never removed silently.** Every run is reported. A whole block is rerun only for a documented external cause (for example Windows Update), and the log says so.
- **Report:** median, IQR, p90, min, max, CV, R with its CI, and every guardrail.

### Harness changes needed (proposed, not implemented)

VERIFIED gaps in `run-benchmarks.mjs`: options are only `--runs`, `--quick` and `--aria2`; there is no `--velocity` and no `--platform-sampler` (revision 1 named a non-existent command); sampling uses `/proc` and `getconf`; the curl runner calls `sh -c`; bytes sent are not counted; and the run order is fixed.

1. A `--engine <path>` runner for a small CLI built from `velocity-transfer`, with flags matching each contender's caps. Both deployment modes must be tested.
2. Randomized interleaved blocks, warm-up, and `--seed`.
3. Server counters for bytes sent, HEAD and GET requests, per-connection and per-IP throttle modes, `429`/`503` with `Retry-After`, malformed ranges, object change, gzip, and an h2/TLS server using Node's built-in `http2` and `tls` modules with a test CA. No new npm dependency.
4. FTP and SFTP servers. Reusing the Rust FTP fixture from `tests/common/ftp.rs` needs a small bench binary. An SFTP server needs OpenSSH, which is a platform dependency requiring approval.
5. **Native Windows resource measurement.** The recommended route is a small Rust `bench-runner` that starts each contender inside a **Job Object** and reads `JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION` (exact user and kernel CPU, read and write bytes) and `PeakJobMemoryUsed`. This is exact for the whole process tree, with no sampling. On Linux the same runner uses `wait4` rusage plus `/proc/<pid>/io`. A direct `windows-sys` dependency (already in `Cargo.lock` transitively) needs approval. A fallback with no code is one long-lived PowerShell sampler streaming `Get-Process` every 100 ms; it is approximate and misses short-lived children.
6. Replace the `sh -c` curl runner with one spawn per file, so it works on Windows.

### Reproducing on the PO's Windows PC (after items 1 to 6 exist)

1. Record the Windows build, CPU, RAM, disk model, power plan and Defender state in the results metadata.
2. Use the High performance plan, on mains power, with other heavy apps closed. Put the target folder on one SSD for all contenders. A Defender exclusion *for the benchmark folder only* may be used, and must be recorded.
3. Run the pilot, then the measured run with `--engine` pointing at the prototype and `--aria2` at the vendored `velocity-aria2c.exe`. Keep `scripts/bench/results/*.json`.
4. **No Windows number is claimed until this procedure has actually been run and its output kept.**

### Functional and security tests

- Parameterize `engine_integration.rs` by `VELOCITY_TEST_HTTP_ENGINE`, as is done for torrents, so both engines pass the same 36 tests.
- The section 5.4 fixtures, the I1–I7 property tests, fuzz targets for `Content-Range`, `Content-Disposition` and Metalink, and a secret-leak scan of the database, logs, checkpoints, temporary files and child argv.
- TLS: the existing certificate-purpose, host-name, expiry and untrusted-CA tests, run against each TLS backend on Linux and on real Windows.

## 8. Security, licensing and packaging

**Revised (R1, R7): a static libcurl 8.23.0 or later, with Schannel on Windows or a static OpenSSL on Linux and macOS, plus nghttp2 and zlib, has no copyleft component in what Velocity distributes. A distro-provided dynamic libcurl does, if it is bundled. Velocity keeps its GPL obligations for every release that ships aria2.** This is an engineering assessment, not legal advice. The final closure must come from `cargo tree`, the native build logs and `scripts/generate-notices.mjs` on the actual builds.

### 8.1 libcurl version and dependency-selection policy (R1)

VERIFIED facts, 2026-10-21:

| Item | Value | Source |
| --- | --- | --- |
| Latest curl release | **8.23.0**, 2026-10-14; tag `curl-8_23_0` → commit `3163ec06` | [curl changes](https://curl.se/changes.html), GitHub tag API |
| `curl` crate | 0.4.51 (2026-10-08), MIT | crates.io API |
| `curl-sys` | 0.4.91+curl-8.22.0. Its `curl` submodule is commit `01346829`, **identical to tag `curl-8_22_0`** (git compare: identical) | crates.io API, GitHub compare |
| Advisories with "last affected" 8.22.0 | **21**. Opened and confirmed fixed in 8.23.0: [CVE-2026-92392](https://curl.se/docs/CVE-2026-92392.html) (**High**, HTTP/2 UAF on upload; Velocity downloads, so exposure is INFERRED low), [CVE-2026-86414](https://curl.se/docs/CVE-2026-86414.html) (host credentials leaked to an HTTP proxy), [CVE-2026-87865](https://curl.se/docs/CVE-2026-87865.html) (libssh2 known-hosts mismatch reported as "missing"), [CVE-2026-86410](https://curl.se/docs/CVE-2026-86410.html) (curl *tool* only: a redirect to SFTP skips host checks). The other 17 are fixed per the [vulnerability table](https://curl.se/docs/vuln-8.22.0.html) "last affected 8.22.0" column (INFERRED fixed in 8.23.0) | curl advisories |
| Upgrade to 8.23.0 | curl-rust PR #692, **open, unmerged** | GitHub |
| Bundled Linux build uses `select()` | curl-rust issue #684 and PR #685, **open**. Connections fail once a descriptor number reaches 1024 | GitHub |
| Segfault when dropping `Multi` with active transfers | Fixed by #645, merged 2026-10-10, **after** 0.4.51 | GitHub |

**Policy:**

1. **Floor:** libcurl 8.23.0. A build below the floor, or one missing a required feature, refuses to start the engine. This is checked at runtime through `curl_version_info` (version, TLS backend, protocols, features).
2. **Build path:** the identity of the libcurl that ships must be provable. In order of preference:
   - **(a) Velocity builds libcurl itself** from the official `curl-8.23.0.tar.xz`, verified by SHA-256 and the release signature. It is recorded in a lock file like `scripts/aria2/deps.lock`, and `curl-sys` links it instead of building its own copy. This reuses the provenance approach already used for aria2. It is the only path that gives SFTP (with libssh2) and HTTP/3 later.
   - **(b) An upgraded `curl-sys` release**, once #692 and #685 are merged. It is simpler, but SFTP is still missing.
   - **(c) A `[patch.crates-io]` pin to a reviewed curl-rust commit.** This is a temporary dependency decision for the prototype only.

   A distro dynamic libcurl is acceptable only for distribution-maintained packages, and needs a runtime feature check.
3. **Identity record:** each build's `build-info.json` records the libcurl source SHA-256, the tag commit, the configure or CMake options, the TLS backend, nghttp2, zlib and libssh2 versions, and the binary checksum. It is copied into the installer's build info, as is already done for aria2.
4. **Release blockers:**
   - libcurl below the floor.
   - Any unfixed advisory rated Medium or higher that affects an enabled protocol or feature.
   - The `select()` build defect present.
   - A missing identity record.
   - An SSH backend without the CVE-2026-87865 fix, when SFTP is enabled.
5. **Updates:** subscribe to curl's security announcements and the libssh2, nghttp2 and OpenSSL advisories. Target response times (a PO decision): High within 7 days, Medium within 30 days, Low in the next release. Each floor bump is its own reviewed change.

### 8.2 Dependency closure per platform (R7)

| Component | Proposed version | Windows x64 | Linux x64 | macOS | Licence | Distribution obligation |
| --- | --- | --- | --- | --- | --- | --- |
| libcurl | 8.23.0 or later | Static | Static | Static | curl (MIT-like) | Keep the notice |
| `curl` / `curl-sys` | 0.4.51 / 0.4.91 or later | Yes | Yes | Yes | MIT | Notice (generator) |
| nghttp2 (`libnghttp2-sys` 0.1.13+1.68.1) | 1.68.1 | Static | Static | Static | MIT | Notice |
| zlib (`libz-sys` 1.1.30) | Bundled source, or system | Static | Static or system | System | Zlib; crate MIT/Apache | Notice is good practice |
| TLS: Schannel | OS | Yes | — | — | Microsoft OS component, not redistributed | None |
| TLS: OpenSSL (`openssl-sys`, vendored `openssl-src`) | 3.x or 4.x, depending on `openssl-sys`'s constraint (the latest `openssl-src` is 400.0.2+4.0.3; which one resolves is NOT checked) | Option (`windows-static-ssl`; libtorrent already links OpenSSL 3 there) | Static | Static, plus `apple-sectrust` (Security.framework) | Apache-2.0 | Licence text plus NOTICE |
| TLS alternative: rustls via `rustls-ffi` 0.15.6 | — | Option | Option | Option | Apache-2.0 OR ISC OR MIT, plus a crypto provider (ring or aws-lc-rs, with their own licences) | Not recommended for the first version |
| SFTP option A: libssh2 | `libssh2-sys` 0.3.3 bundles master `4aded1bf` (unreleased); last release 1.11.1 | Static (WinCNG crypto) | Static (OpenSSL crypto) | Static | BSD-3-Clause | Notice |
| SFTP option B: russh 0.64.1 + russh-sftp 3.0.1 | — | Yes | Yes | Yes | Apache-2.0 (crypto dependency closure NOT TESTED) | Notice |
| HTTP/3 (DEFERRED): ngtcp2, nghttp3 | — | — | — | — | MIT; needs OpenSSL 3.5 or later | Notice |
| C runtime | — | MSVC runtime, as the app uses today | glibc, a system library dynamically linked and not shipped in `.deb`/`.rpm` | libSystem | Microsoft terms / LGPL-2.1+ (system) / Apple | Unchanged from today |
| **Distro libcurl (avoid bundling)** | Ubuntu 24.04: 8.5.0 | — | Dynamic. Links OpenSSL 3.0.13, brotli, zstd, **libidn2** (LGPLv3+ or GPLv2+), libpsl, **libssh 0.10.6 (LGPL-2.1)**, nghttp2, **librtmp (LGPL-2.1)** and **OpenLDAP** (VERIFIED from the saved benchmark's `curl -V` line) | — | Mixed, including LGPL | LGPL obligations (relinking, source) **if an AppImage bundles them**. Bundle `targets` is `"all"` |

Conclusion: the "no copyleft" claim holds for Windows (static libcurl, Schannel, nghttp2, zlib) and for Linux and macOS (static libcurl, static OpenSSL, nghttp2, zlib), with either SFTP option. glibc is a system library on Linux. It does **not** hold for an AppImage that bundles a distro libcurl and its dependencies.

### 8.3 What removing aria2's GPL obligations would take

- **Using libcurl does not remove them.** As long as any installer ships `aria2c`, every LICENSING.md obligation applies to that release: GPL text, notices, and complete corresponding source or a written offer.
- **Exit conditions:**
  1. A release whose installers contain no aria2 binary.
  2. The app shows "needs the legacy engine" for any remaining aria2 records, rather than bundling aria2 for them.
  3. Notices and README updated.
- **Obligations already incurred continue.** A written offer made for an earlier release must stay valid for at least three years ([GPLv2 §3(b)](https://www.gnu.org/licenses/old-licenses/gpl-2.0.en.html)).
- **The repository stays mixed-licence** while `vendor/aria2/` is present. Removing it from source is a separate decision.
- Qualified licensing review is still required (`docs/LICENSING.md`).

### Security controls the engine must enforce

- **Protocol allowlists.** *Threat:* a redirect sending the engine to `file:`, `scp:` or `sftp:`. *Policy:* `CURLOPT_PROTOCOLS_STR` is `http,https,ftp` (plus `ftps` and `sftp` when enabled), and `CURLOPT_REDIR_PROTOCOLS_STR` is `http,https`. A redirect can therefore never reach SFTP, which also sidesteps the class behind CVE-2026-86410 (a tool-only flaw). https→http downgrades are refused unless the user confirms. *Existing contract:* aria2 follows redirects within its own protocols.
- **TLS.** Keep `SSL_VERIFYPEER` and `SSL_VERIFYHOST` on; never offer a switch to disable them. Use the platform trust store. Rerun the CVE-2026-8367 certificate-purpose, host-name and expiry tests per backend. Revocation behaviour differs by backend (Schannel against OpenSSL). Document it; do not invent a policy (NOT TESTED).
- **Credentials and redirects.** *Threat:* credentials or cookies leaking to another host. Supplied credentials are sent only to the original host: libcurl's default unless `CURLOPT_UNRESTRICTED_AUTH` is set, which Velocity never sets (INFERRED from the docs). Cookies and custom headers are stripped when a redirect leaves the origin, by Velocity's own rule. Use libcurl 8.23.0 or later for the Negotiate and proxy fixes (CVE-2026-94265, CVE-2026-86413, CVE-2026-86414).
- **Proxies.** Set the proxy explicitly from settings, using an empty string for "none", so ambient `http_proxy` variables have no effect. Never enable `.netrc`. Proxy credentials are kept in memory or the OS keychain.
- **SFTP host keys.** *Threat:* a man-in-the-middle presenting a new key. *Policy:*
  - Use a Velocity-owned, **unhashed** known\_hosts file (CVE-2026-87865 involved hashed entries), plus a SHA-256 pin per host (`CURLOPT_SSH_HOST_PUBLIC_KEY_SHA256`, or `check_server_key` in russh).
  - An unknown key is refused until the user confirms its fingerprint. A changed key is **always** refused.
  - Keys and passphrases are kept in memory or the OS keychain, never in the database.
- **Local and private network targets.** *Threat:* a web page making the browser hand Velocity a link to `127.0.0.1` or a LAN device (SSRF), including through DNS rebinding. *Existing contract:* no such check exists today (NOT TESTED). *Proposal:* check the connected IP in `CURLOPT_PREREQFUNCTION`, applied to browser-originated requests only. Whether to block, warn or allow is a PO decision (section 10).
- **Files.**
  - Use the existing `validation.rs` naming and containment.
  - Open `.vdmpart` and checkpoint files without following links or reparse points (`O_NOFOLLOW`, `FILE_FLAG_OPEN_REPARSE_POINT`), with mode 0600 for checkpoints.
  - Delete only paths the record owns.
  - Checkpoints store a URL *fingerprint*, not query tokens. The full URL stays in the database, as today.
- **Secrets in diagnostics.** Pass credentials by `setopt` (in-process) or through the stdio pipe (sidecar), never on argv or env. Route curl's debug callback through `redact.rs`. Extend the existing session-scrub test into a secret-leak scan.
- **Cancellation.** Add and remove easy handles only on the `Multi` thread. A cancelled transfer's file is closed before any deletion. Lease revocation (section 5.2) guarantees there are no late writes.
- **Engine messages as untrusted** (sidecar model): validate size, schema and version on the app side, the same rule as for aria2 responses today.

### Packaging

- **Windows:** a Velocity-built static libcurl with Schannel, nghttp2 and zlib, and libssh2 (WinCNG) if option A is chosen. Alternatively, an upgraded `curl-sys` with `static-curl` and `ssl` (Schannel), once #692 is merged. Real Windows verification is required; Wine is not enough. A sidecar model adds a third signed `externalBin`.
- **Linux:** static libcurl for `.deb`, `.rpm` and AppImage. Do not bundle the distro libcurl into an AppImage (section 8.2). Distribution packagers may link the system libcurl, with the runtime feature check.
- **macOS:** not established in the project. Secure Transport was [removed in curl 8.15.0](https://daniel.haxx.se/blog/?p=27656), so the route is a static OpenSSL plus `apple-sectrust`. It needs a first build and test.
- **aria2** stays a sidecar with unchanged GPL obligations until section 8.3's exit conditions are met.

## 9. Feature and delivery roadmap

**Eleven stages (revised). Each is a separate PR or a small PR series that can be tested and rolled back on its own. Two explicit approval points: after stage 3 (go or no-go on the engine, deployment model and SFTP approach) and after stage 10 (default change).** aria2 stays the default until stage 10's gate is passed.

### Feature tiers (revised)

- **Launch requirements:**
  - HTTP/1.1 and HTTPS, FTP, and **SFTP** (now required).
  - The bounded probe; leased segmentation; validator-checked resume.
  - Resume after pause, crash and restart.
  - Queue and concurrency; per-download and global speed limits.
  - User checksums with a whole-file pass; retry classification with `Retry-After`.
  - Conflicts and naming (reused); in-memory credentials; strict TLS and SFTP host keys; redacted diagnostics.
- **Enhancements that could beat aria2** (each needs its own benchmark or test before it is claimed):
  - Detection of a changed server file.
  - Live connection changes without a restart.
  - Measured ramp-up, and finer work-stealing.
  - HTTP/2; TLS 1.3 on Windows.
  - Atomic finalization without replacing.
  - Connection reuse across small files.
  - SOCKS5 and authenticated proxies.
- **Advanced parity:** FTPS, Metalink v3/v4 with piece hashes, mirror failover.
- **Deferred:** HTTP/3, DoH, persistent per-host learning, in-order "streaming" mode, `Repr-Digest`, OS sandboxing of the engine, and moving existing aria2 downloads.

### Stages

| # | Stage | Depends on | Main risk | Exit gate | Tested by |
| --- | --- | --- | --- | --- | --- |
| 0 | PO decisions (section 10) | This report | Scope creep | Written decisions on the items marked "now" | Review |
| 1 | **Parser fix** (`EngineKind::Unavailable`) plus the `DirectTransfer` seam with the `Aria2Direct` adapter | 0 | Regressions in the manager | Zero behaviour change: `engine_integration` 36/36, `torrent_integration`, Vitest and the contract fixtures all unchanged. A new test: an unknown engine value is never resubmitted | Existing suites plus a unit test |
| 2 | Benchmark harness v2 (section 7, items 1 to 6) with contenders A, B and C | 0 | A biased harness | Pilot CV measured; A, B and C published for Linux and the PO's Windows PC with Job Object accounting | The harness run itself |
| 3 | Prototype: `velocity-transfer` + `velocity-curl` on libcurl 8.23.0 or later, HTTP(S) only, in-process **and** sidecar | 1, 2 | Throughput, or subtle correctness bugs | Section 7 gates on loopback (Linux and Windows), I1–I7 property tests, and the HTTP fixtures in section 5.4. **Then a PO go or no-go** | Bench v2, simulation, fixtures |
| 4 | Core engine: checkpoints, recovery, finalization, retries, global limiter, Velocity-owned queue | 3 + go | Corruption on crash | `engine_integration` parameterized for both engines; 100 random `kill -9` trials | Real-engine tests |
| 5 | FTP and FTPS | 4 | Server quirks | The existing FTP tests on the new engine; an FTPS fixture | Local FTP fixture |
| 6 | SFTP bake-off: libssh2 through a Velocity-built libcurl, against russh | 4 | Host-key UX; maintenance | The section 5.4 SFTP rows pass on both; the choice is recorded with its evidence | Local OpenSSH (needs approval) |
| 7 | Opt-in setting, migration 0004, mixed-engine routing | 4–6 | Mixed-engine bugs | Section 6 rollback table tested, including a downgrade refusal | Migration and integration tests |
| 8 | Security hardening | 4 | A missed leak | Fuzz targets clean; leak scan clean; SSRF policy implemented as decided; TLS tests on real Windows | cargo-fuzz, scans, Windows run |
| 9 | Metalink and mirrors | 4, 8 | XML and path attacks | RFC 5854 corpus plus malicious names; piece-hash re-fetch | Corpus fixtures |
| 10 | Packaging and production acceptance | 7, 8 | Platform TLS differences | `build-info` identity, notices regenerated, a soak test of at least 300 runs per key scenario, NSIS install, documented rollback. **Then a PO decision on the default** | Release checklist |

After stage 10, aria2 stays bundled as the legacy engine for unfinished aria2 records. Removing it is a separate later decision (section 6).

## 10. Product Owner decisions needed

**Ten decisions (revised). Four are needed now to start stages 1 to 3. The rest wait for prototype evidence.**

| # | When | Decision | Options and consequences | Recommendation |
| --- | --- | --- | --- | --- |
| 1 | Now | Start a Velocity transfer engine at all, while issue #18 is pending | (a) Yes, stages 1 to 3 only: stage 1's parser fix and the benchmark harness are useful either way. (b) Stay on aria2: no new risk, and the section 1 gaps remain | (a) |
| 2 | Now | libcurl build path for the prototype | (a) Velocity builds libcurl 8.23.0 itself: provable identity, SFTP and HTTP/3 possible; more build work. (b) Wait for `curl-sys` with 8.23.0 and the poll fix. (c) A temporary git pin to a reviewed curl-rust commit: fastest, prototype only | (c) for the prototype; (a) for production |
| 3 | Now | Benchmark tooling | Approve the harness changes, the Rust Job Object runner (a direct `windows-sys` dependency), and runs on the PO's Windows PC | Approve |
| 4 | Now | Engine logic in a new `velocity-transfer` crate rather than `velocity-core` | A new crate allows a sidecar binary without pulling in SQLite and Tauri types, but deviates from "core owns engine logic" | New crate, with `velocity-core` keeping policy and adapters |
| 5 | After stage 3 | Deployment model | In-process: simplest, but a native fault or panic ends the app. Child process over stdio: isolation and restart, at a measured cost | Child process, unless it misses the cost hypotheses in section 4.3 |
| 6 | After stage 3 | Go or no-go on the full engine | Based on the section 7 gates | Decide on evidence |
| 7 | Stage 6 | SFTP backend | libssh2 (BSD-3; no release since 2024; needs a Velocity-built libcurl) against russh (Apache-2.0; pure Rust; a younger security record) | Bake-off on the section 5.4 tests |
| 8 | Stage 4 | Resume without a validator | (a) Resume with a length check plus a "not validated" flag: aria2's behaviour, no regression. (b) Restart from 0 unless a checksum is given: safer, but costs bandwidth | (a), plus the visible flag |
| 9 | Stage 8 | Browser links to loopback or private addresses | Block, warn and confirm, or allow | Warn and confirm, for browser-originated requests only |
| 10 | Stage 7 | Legacy aria2 downloads | Keep them on aria2 (safe), or offer "Restart with the new engine" for zero-byte records | Keep them; add the action only if users ask |

Not decided here: Velocity's own licence (issue #4) and advisory response times (section 8.1), although this report changes the inputs to both.

Not a decision for this report: Velocity's own licence (issue #4), though section 8 changes its inputs.

## 11. Source register

**Nothing was built, installed, benchmarked or run against a network for this report.** Repository facts come from reading files at `develop` `e9d717a8`. External facts come from the pages below, opened on 2026-10-21.

### Repository evidence (VERIFIED by reading)

| Claim | Location |
| --- | --- |
| aria2-shaped engine API and GID routing | `crates/velocity-core/src/engine/router.rs` |
| libtorrent imitates `Aria2Status` | `crates/velocity-core/src/libtorrent/engine.rs:21, 438` |
| Submit, reconcile and finalize logic | `crates/velocity-core/src/manager.rs:962, 1889, 2250` |
| Optimizer limited to aria2 options; 16 per server | `crates/velocity-core/src/optimizer.rs` |
| SFTP refused | `crates/velocity-core/src/validation.rs:75` |
| Record schema, engine column, states | `crates/velocity-core/src/database/downloads.rs`, `migrations/0001`–`0003` |
| reqwest built without TLS | `crates/velocity-core/Cargo.toml` |
| aria2 HTTP/1.1 only; range validation; no ETag/If-Range | `vendor/aria2/src/HttpRequest.cc:167`, `HttpResponse.cc:71`, grep of `src/` |
| aria2 defaults: `min-split-size` 20M, `piece-length` 1M, at most 16 per server, `uri-selector` feedback | `vendor/aria2/src/OptionHandlerFactory.cc:439, 502, 906, 1010` |
| Vendored builds without libssh2; Linux GnuTLS, Windows WinTLS | `scripts/aria2/build-linux-x64.sh`, `build-windows-x64.sh` |
| Benchmark method and latest results | `docs/PERFORMANCE_BENCHMARKS.md`, `scripts/bench/results/bench-2026-10-18T12-52-26-548Z.md` |
| Phase 5 allows Rust replacement behind a contract | `docs/ARIA2_FORK_STRATEGY.md` |
| Issue #35 scope and evidence comment | GitHub issue #35, read with `gh` on 2026-10-21 |

Added in revision 2 (all VERIFIED by reading at `e9d717a8`):

| Claim | Location |
| --- | --- |
| Unknown `engine` values parse as aria2 | `crates/velocity-core/src/database/downloads.rs:139-142` |
| A newer schema is refused unmodified | `crates/velocity-core/src/database/mod.rs:65` |
| `panic = "abort"`, `lto = true` in release | workspace `Cargo.toml` |
| About 55 router call sites, 18 `Aria2Status` uses, aria2 option names | grep over `manager.rs` and `manager/torrent.rs` |
| Sidecars `velocity-aria2c` and `velocity-native-host`; bundle targets `"all"` | `src-tauri/tauri.conf.json:32, 46` |
| Harness options, `/proc` sampling, `sh -c` curl runner, fixed order, no byte counters | `scripts/bench/run-benchmarks.mjs:15-35, 127-200, 205-300` |
| Saved results: small files, no-range requests, the curl 8.5.0 dependency line | `scripts/bench/results/bench-2026-10-18T12-52-26-548Z.md` and `.json` |
| aria2 decodes gzip/deflate | `vendor/aria2/src/HttpResponse.cc:206-216` |

### External sources (official documentation unless noted)

- [curl licence](https://curl.se/docs/copyright.html) and [changes](https://curl.se/changes.html): 8.23.0 released 2026-10-14; the [man page](https://curl.se/docs/manpage.html) shows 8.23.1. `--parallel` fetches several URLs concurrently, not one file in segments, and Metalink is disabled since 7.78.0.
- [libcurl multi interface](https://curl.se/libcurl/c/libcurl-multi.html), [CURLOPT\_RANGE](https://curl.se/libcurl/c/CURLOPT_RANGE.html), [CURLMOPT\_PIPELINING](https://curl.se/libcurl/c/CURLMOPT_PIPELINING.html), [CURLMOPT\_MAX\_HOST\_CONNECTIONS](https://curl.se/libcurl/c/CURLMOPT_MAX_HOST_CONNECTIONS.html), [CURLOPT\_MAX\_RECV\_SPEED\_LARGE](https://curl.se/libcurl/c/CURLOPT_MAX_RECV_SPEED_LARGE.html), [CURLOPT\_SSH\_KNOWNHOSTS](https://curl.se/libcurl/c/CURLOPT_SSH_KNOWNHOSTS.html)
- [curl HTTP/3](https://curl.se/docs/http3.html), [TLS backends compared](https://curl.se/docs/ssl-compared.html), [curl security advisories](https://curl.se/docs/security.html), [curl 8.15.0 release (Secure Transport removed)](https://daniel.haxx.se/blog/?p=27656) (the maintainer's blog)
- [`curl` crate 0.4.51](https://docs.rs/crate/curl/latest), [`curl-sys` 0.4.91 features](https://docs.rs/crate/curl-sys/latest/features), [`curl-sys` build.rs](https://raw.githubusercontent.com/alexcrichton/curl-rust/main/curl-sys/build.rs) (source)
- [reqwest 0.13.5](https://docs.rs/reqwest/latest/reqwest/), [suppaftp 12.2.1](https://docs.rs/crate/suppaftp/latest), [russh-sftp 3.0.1](https://docs.rs/crate/russh-sftp/latest) (licence not shown on that page: to be checked), [libssh2](https://libssh2.org/)
- [GNU Wget2 README](https://gitlab.com/gnuwget/wget2/-/raw/master/README.md), [Axel](https://github.com/axel-download-accelerator/axel), [Gopeed](https://raw.githubusercontent.com/GopeedLab/gopeed/main/README.md), [Motrix](https://raw.githubusercontent.com/agalwood/Motrix/master/README.md), [Persepolis history](https://videohelp.com/software/Persepolis-Download-Manager/version-history) (secondary source)
- [IDM features](https://www.internetdownloadmanager.com/features2.html), [FDM features](https://www.freedownloadmanager.org/features.htm)
- HTTP semantics for ranges, `If-Range` and `Retry-After`: [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110). Metalink: [RFC 5854](https://www.rfc-editor.org/rfc/rfc5854) and Metalink/HTTP [RFC 6249](https://www.rfc-editor.org/rfc/rfc6249). Cited as standards; not re-read for this report.

Added in revision 2 (opened 2026-10-21; VERIFIED unless noted):

- curl advisories: [8.22.0 vulnerability table](https://curl.se/docs/vuln-8.22.0.html), [security index](https://curl.se/docs/security.html), [CVE-2026-92392](https://curl.se/docs/CVE-2026-92392.html), [CVE-2026-86414](https://curl.se/docs/CVE-2026-86414.html), [CVE-2026-87865](https://curl.se/docs/CVE-2026-87865.html), [CVE-2026-86410](https://curl.se/docs/CVE-2026-86410.html). The remaining 17 advisories are cited from the table only; their pages were not opened.
- curl-rust ([repository](https://github.com/alexcrichton/curl-rust)): PR #692 (8.23.0, open), issue #684 and PR #685 (`select()`, open), PR #645 (segfault fix, merged 2026-10-10), PR #663 (timeout overflow, merged). The submodule `01346829` is identical to `curl-8_22_0`; `curl-8_23_0` is `3163ec06` (GitHub API).
- crates.io API: `curl` 0.4.51, `curl-sys` 0.4.91+curl-8.22.0, `libnghttp2-sys` 0.1.13+1.68.1, `libz-sys` 1.1.30, `openssl-src` 400.0.2+4.0.3, `rustls-ffi` 0.15.6, `ssh2` 0.9.6, `libssh2-sys` 0.3.3, `russh` 0.64.1, `russh-sftp` 3.0.1, with their licences.
- libssh2 ([repository](https://github.com/libssh2/libssh2)): last release 1.11.1 (2024-10-16); `libssh2-sys`'s submodule `4aded1bf` (2026-08-04) is 948 commits after it.
- russh: [`Handler::check_server_key`](https://docs.rs/russh/latest/russh/client/trait.Handler.html), [RUSTSEC-2026-0154](https://rustsec.org/advisories/RUSTSEC-2026-0154.html), [russh-sftp `File`](https://docs.rs/russh-sftp/latest/russh_sftp/client/fs/struct.File.html).
- libcurl: [CURLOPT\_HEADERFUNCTION](https://curl.se/libcurl/c/CURLOPT_HEADERFUNCTION.html), [CURLOPT\_RESUME\_FROM\_LARGE](https://curl.se/libcurl/c/CURLOPT_RESUME_FROM_LARGE.html). The page does not state the behaviour when a server refuses to resume, so that is NOT TESTED.
- [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) text read for §§8.8.2.2, 8.8.3.2, 10.2.3, 13.1.5, 14.2, 14.3 and 15.3.7.1.
- [GPLv2](https://www.gnu.org/licenses/old-licenses/gpl-2.0.en.html) §3(b): a written offer must be valid for at least three years (standard text; not re-read).

### Open research questions (DEFERRED to the prototype)

1. Does Schannel provide TLS 1.3 and pass the certificate-purpose tests on the Windows versions Velocity supports? (NOT TESTED)
2. Does HTTP/2 multiplexing beat separate connections against common CDNs? (NOT TESTED; a section 7 scenario.)
3. Can `CURLOPT_MAX_RECV_SPEED_LARGE` change during a transfer, or must the global limiter pause and unpause? The docs are silent (NOT TESTED).
4. What does libcurl do when a server refuses a resume or range? The `CURLOPT_RESUME_FROM_LARGE` page is silent, so Velocity classifies responses itself (section 5.1) (NOT TESTED).
5. Which OpenSSL major version does `openssl-sys`'s vendored build resolve to, and what is russh's full crypto closure? (NOT TESTED; answered by `cargo tree` in the prototype.)
6. How much does an abort at a split boundary cost on HTTP/1.1 (a closed connection) compared with HTTP/2 (a stream reset)? (NOT TESTED)
7. What is the NTFS zero-fill cost of writing at high offsets without preallocation? (INFERRED, NOT TESTED)
8. Does aria2 flush its disk cache before saving the control-file bitfield? Relevant only if importing `.aria2` state is ever reconsidered (DEFERRED).

## 12. Recommendation

**Approve now:** stage 1 (the engine parser fix and the `DirectTransfer` seam, with zero behaviour change), stage 2 (benchmark harness v2 with Job Object accounting on Windows) and stage 3 (a disposable libcurl 8.23.0+ HTTP(S) prototype, in-process and as a sidecar). Also approve decisions 1 to 4 in section 10. The parser fix is worth shipping even if the engine is never built: it closes a latent mis-routing hazard.

**Needs the prototype before approval:**

- Whether the engine is worth building at all, judged by section 7's gates against the *best* valid aria2 configuration.
- The deployment model.
- The correctness of the leased scheduler (I1–I7) and of the probe and validator rules (section 5.4).
- The SFTP backend, through the stage 6 bake-off.

**Evidence that would justify approving the complete replacement** (stage 10):

1. Zero corrupted or falsely completed files across all benchmark, fixture and soak runs.
2. Non-inferiority to the best valid aria2 configuration in every scenario, on Linux and the PO's Windows PC, with every guardrail met.
3. Superiority in at least the small-file and interrupted scenarios, *or* a written PO acceptance that parity plus the correctness gains are enough.
4. All 36 `engine_integration` tests, the FTP tests and the SFTP host-key tests passing on the new engine.
5. 100 random `kill -9` recoveries without data loss.
6. libcurl at or above the floor, with a provable build identity.
7. A regenerated, reviewed licence closure.
8. A tested rollback across mixed engines.

Until then, aria2 remains the default, and nothing in this report should be read as a performance claim.
