# Engine internals

How the direct-download engine in `crates/velocity-transfer` fetches a file. The contract the manager sees is in [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md), and how the engine's child process talks to the app is in [ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md). The design comes from section 5 of the [research report](research/2026-10-21-libcurl-engine-research-r2.md); the terms are in the [glossary](GLOSSARY.md).

## The probe (#41)

The probe is the download's first request and also its first segment, so its bytes are kept. It is `GET` with `Range: bytes=0-(S−1)`, where S is `DownloadOptions::probe_bytes`. That's 4 MiB in the engine, the report's starting value for #47 to tune.

- **No HEAD:** a HEAD request is never sent, and `Accept-Ranges` is never taken as proof of range support.
- **No encoding asked for:** no `Accept-Encoding` is sent. An encoding the server applies anyway is kept, its bytes stored as sent ([changed files](#changed-files-45)).
- **Decided before any byte:** the final response is classified in libcurl's header callback ([`probe.rs`](../crates/velocity-transfer/src/probe.rs)). An answer the engine refuses is aborted there, so none of its body is written.

| Final response | What the engine does | `Mode` |
|---|---|---|
| `206`, `Content-Range: bytes 0-b/T` | Keeps bytes 0 to b, then fetches the rest as parallel segments | `Ranges` |
| `206` with fewer bytes than asked (b < S−1) | Keeps them; the rest is part of the segments | `Ranges` |
| `206`, `bytes 0-b/*` (no total) | Keeps bytes 0 to b. It asks for the rest with one open range, `bytes=(b+1)-`, and repeats from the first missing byte until a `416` says the file ends there. No parallel segments | `SequentialRanges` |
| `206` `multipart/byteranges` | Aborts, and retries once without `Range` | `SingleStream` after the retry |
| `200` | Keeps this response as the whole file: no second request, nothing wasted | `SingleStream` |
| `416` with `bytes */0` | An empty file | `Empty` |
| `401`, `407` | Fails: the server or proxy requires authentication. Never retried | — |
| `429`, `503` | Throttled: retried after the wait `Retry-After` asks for ([retries](#retries-waits-and-stalls-48)) | — |
| Other `4xx` | Fails, never retried (`408` is retried) | — |
| Other `5xx` | Retried after a wait | — |
| No answer: the connection failed | Retried after a wait | — |
| A malformed `Content-Range`, a `Content-Length` that disagrees with it, a start other than 0, an end past the request or the total | Fails before writing, not retryable | — |

## Redirects (#40)

Velocity follows redirects itself; libcurl's `CURLOPT_FOLLOWLOCATION` is off. libcurl would follow one with every custom header, and it drops only `Authorization` and `Cookie` when the host changes. When a response is a `3xx` with a `Location`, the libcurl loop drops its body, reads the next URL as libcurl resolved it (`CURLINFO_REDIRECT_URL`), and checks the hop ([`redirect.rs`](../crates/velocity-curl/src/redirect.rs)):

- **Protocols:** only `http` and `https`. A redirect to `ftp`, `sftp`, `scp`, `file` or anything else is refused before any connection. libcurl's `CURLOPT_REDIR_PROTOCOLS_STR` stays set to `http,https` as a second barrier.
- **No downgrade:** a redirect from `https` to `http` is refused. Allowing it after the user confirms is #67.
- **At most 10** redirects per request; the 11th is refused.
- **Origins:** the next request carries the login and the custom headers only when it goes to the first URL's origin: its scheme, host and port. The origin is read with libcurl's own URL parser ([`url.rs`](../crates/velocity-curl/src/url.rs), `curl_url`), so Velocity and libcurl agree on the host. For example, `http://evil.example\@good.example/` goes to `good.example` for both. The user agent and the referrer go with every request.

The same handle is reset and set up again for each hop, so nothing from the last hop carries over. A refusal ends the request as `FetchError::RedirectRefused`, and the download fails with a message that names the protocol, never the URL. Each segment request follows the redirects from the first URL again, so a link that redirects to a short-lived URL works for every segment.

## File names (#40)

A spec without a `fileName` downloads into its folder, and the server's first answer names the file:

1. The `Content-Disposition` header (RFC 6266): `filename*` (RFC 8187, UTF-8 or ISO-8859-1) wins over `filename`. Any path in it is dropped, so `../../x.sh` gives `x.sh`.
2. Else the final URL's last path segment, after redirects, percent-decoded.
3. Else `download`.

The name is then sanitised ([`naming.rs`](../crates/velocity-core/src/naming.rs), ported from V1): separators, control characters and `< > : " | ? *` become `_`, leading dots and trailing dots and spaces go, Windows device names such as `CON.txt` get a `_` in front, a name ending in `.vdmpart` gets one at the end, and the name is cut to 240 bytes, keeping its extension. When the name is taken by a file, a link or a part file, the first free `name (n).ext` is used, n from 1 to 9999 (PO decision on #40), so no file is ever replaced.

- **No extra request:** the part file is created when the probe's first byte arrives, or when the probe ends without a body, as for an empty file. Before that nothing is on disk; a pause then keeps nothing, and the next attempt asks again.
- **Reported:** the engine reports the name in the snapshot's `fileName`. From then on, retries, pauses and cancels use that file.
- **Given names:** a spec that gives `fileName` is saved under it, and it fails with `FileExists` when the name is taken; #31 adds the conflict settings. `naming::file_name_for_link` decides whether a link names its file on its own: `report.pdf` does, `download.php?id=3` and `/files/12345` don't, so those are left to the server. Nothing calls it yet; the add flow that will is R8's.

## FTP and FTPS (#51)

`ftp://` and `ftps://` links download through the same engine as HTTP. FTP has no response head, so the libcurl layer builds one from the server's control replies before the first body byte ([`ftp.rs`](../crates/velocity-curl/src/ftp.rs)). The probe, the gate, the validators, the checkpoints and the completion gate then work as they do for HTTP.

### The head from the replies

libcurl passes FTP's control replies to the header callback (`213 1234` for a `SIZE`), and only its debug output shows which command each reply answers. The libcurl layer turns debug output on for FTP links only, reads the verb of each command sent (`SIZE`, `MDTM`, `REST`, `RETR`), and pairs it with the next reply. It keeps nothing else from that output, so the `PASS` line never leaves the callback.

| What the replies say | The head | When |
|---|---|---|
| `RETR` started (`150` or `125`), no range asked | `200`, `Content-Length` from `SIZE` | At the `RETR` reply |
| `RETR` started, a range from byte a, `SIZE` gave T | `206`, `Content-Range: bytes a-b/T`, where b is the range's end or T−1 | At the `RETR` reply |
| A range from byte a > 0, and `SIZE` says T ≤ a | `416`, `Content-Range: bytes */T`. libcurl doesn't retrieve such a file, and the fetch ends without a body, like an HTTP `416` | At the `SIZE` reply |
| Byte 0 of an empty file (T = 0) | `416`, `bytes */0`: an empty file, as for HTTP | At the `RETR` reply |
| `REST` refused (any reply but `350`) | `200`: the server can't start part way | At the `REST` reply |
| A range, but no size (`SIZE` refused) | `200` without a length: nothing proves the range | At the `RETR` reply |
| `MDTM` gave a time | `Last-Modified`, with a `Date` of now | Added to the head |
| `RETR` failed: `550` | No head; the fetch fails as `NotFound` | — |

- **A range is `REST`:** the start is sent as `REST` (`CURLOPT_RESUME_FROM_LARGE`), and the range's end is the fetch's body limit, which stops the transfer there.
- **No byte from another offset:** a `200` for a range after byte 0 is aborted before any byte, whatever the gate says, because its bytes would start at the `REST` offset and not at byte 0. A body that arrives before any head is refused too.
- **The date as a validator:** `MDTM` (RFC 3659 §3) is in UTC, to the second. With the head's `Date` of now, it is a strong date once it is at least a second old ([changed files](#changed-files-45)), so a resume is checked against it as `Last-Modified` is for HTTP. A file whose time changed since its checkpoint starts over. Without `MDTM`, a resume is kept on its size alone and shows "not verified" ([ADR-0010](decisions/0010-resume-without-validator.md)). FTP has nothing like `If-Range`: the time is compared in the gate, before any byte is written.

### One connection, one stream

An FTP download uses one connection, whatever is asked (PO decision on #51): each FTP transfer opens a control and a data connection, and nobody has measured yet whether more help (#77). `set_connections` returns 1 for it. `DownloadOptions::one_connection` makes the probe ask for the whole file, so a download that isn't cut is one transfer. What a cut, a pause or a crash leaves is one lease to the end, so one `REST`. A server that refuses `REST` sends the whole file again, and only the missing bytes are written (`RangesLost`). The per-host cap counts FTP on port 21 and FTPS on 990 unless the link names another.

### Logins and encryption

- **The login** comes from the spec's `credentials` and goes out as FTP's `USER` and `PASS`. Without one, libcurl logs in as `anonymous`. A link with a login in it is refused by the engine; the app splits it off first and never stores it (`database::without_login`). FTP has no redirects, so the origin rules don't apply.
- **Plain FTP sends the login and the file in clear.** The engine reports what it can encrypt per protocol (`ProtocolCapabilities::encryption`), and the add form (R8) shows `cleartextWarning` from `src/capabilities.ts` before it starts a plain `ftp://` download (PO decision on #51).
- **Explicit FTPS:** a download that requires encryption (`TransferSpec::require_encryption`) asks `AUTH TLS` before the login, then `PBSZ 0` and `PROT P`, so the data connections are TLS too (`CURLUSESSL_ALL`, `CURLFTPAUTH_TLS`). A server that refuses fails the download at once, as `EncryptionRefused`, before the login is sent. There is no fallback to plain FTP.
- **Implicit FTPS:** an `ftps://` link is TLS from the first byte, on both connections.
- **Certificates** are verified as for HTTPS, with the platform's trust store ([SECURITY.md](SECURITY.md#tls-verification-69)). A refused one fails at once, before the login.
- **Passive mode only,** to the control connection's address: the address in a `PASV` reply is ignored (`CURLOPT_FTP_SKIP_PASV_IP`), so a server can't send the data connection to another host.
- **An `http://` link that requires encryption is refused** (`InvalidInput`): it can't be encrypted.

### Failures

| What happened | Kind | Retried |
|---|---|---|
| The file isn't there (`550`) | `NotFound`: "The file wasn't found on the server." | No |
| The login was refused (`530`), or access to the file | `AccessDenied` | No |
| Encryption is required, and `AUTH TLS` was refused | `EncryptionRefused` | No |
| The data connection broke part way | `Interrupted` | Yes, from the first missing byte, with `REST` |

### FTPS: the evaluation #51 asked for

#51 asked for a written evaluation of FTPS with a recommendation. The PO decided to build it in #51, explicit and implicit, and #22, which the issue names, is the closed parity inventory, so the evaluation is here.

- **Certificate verification (verified):** FTPS uses the same TLS stack as HTTPS: Schannel with the Windows store on Windows, OpenSSL on Linux. The tests run explicit and implicit FTPS against the test CA of #69 on both, and a refused certificate, an untrusted issuer or an expired leaf, fails before the login. The FTP server presents a [`TestPki`](../crates/velocity-test-http/src/tls.rs) leaf, with its revocation list served, as the HTTPS front does.
- **Server compatibility (partly verified):** explicit FTPS with `AUTH TLS` (RFC 4217) is the common form, and implicit FTPS on port 990 is the older one; both work against the test server. Not verified: some servers require the data connection to resume the control connection's TLS session (vsftpd's `require_ssl_reuse`, for example). libcurl reuses TLS sessions by default, but the test server doesn't require it, so that combination has no test. `CCC` (dropping TLS after the login) isn't used.
- **Cost (not measured):** each FTPS transfer adds a TLS handshake on the control connection and one on the data connection. No measurement exists (ADR-0009); #77 measures.
- **Recommendation:** as built. Require encryption wherever the server offers FTPS, and keep plain FTP for servers that don't, with the warning.

## SFTP (#53)

`sftp://` links download on russh, in `crates/velocity-sftp` ([ADR-0013](decisions/0013-sftp-russh.md)). The engine fetches through one interface, `velocity_curl::Transport`, so SFTP shares everything after the connection with HTTP and FTP: the probe, the gate, checkpoints, retries, speed limits and the completion gate.

### One transport interface

- **`Transport`:** the engine's downloads take a `&dyn Transport`. In the engine, a router sends `sftp://` links to the SFTP transport and every other link to libcurl's loop.
- **`FetchPipe`:** a transport other than libcurl writes its fetch through one, with the same events as libcurl's (a head through the gate, the body, then the end). It has the same reader's buffer, speed limits and body limit as libcurl's write callback; both use one `Sink` (`velocity-curl/src/pipe.rs`). A range's end is the body limit.
- **`file_head`:** the head an HTTP server would send for a file of a known size and time (`velocity-curl/src/head.rs`), shared by FTP and SFTP: `200`, `206` for a range, `416` past the end, `Last-Modified` from the file's time with a `Date` of now.

### A fetch

Each SFTP fetch is a task on the transport's own tokio runtime (2 worker threads):

1. **Connect,** within 30 s (as libcurl's connect timeout), to the link's host and port, 22 by default.
2. **The host key** is checked against V2's known_hosts file before anything else is sent ([below](#host-keys)).
3. **Log in** as the user of the spec's `credentials`: with `sshKey` (a private key file and its passphrase) when the spec has one, otherwise with the password.
4. **`stat`** gives the file's size and modification time, which make the head. The gate decides before a byte is read.
5. **Read** from the range's start, 64 KiB at a time, through the `FetchPipe`. A full buffer waits for the reader; a speed limit without tokens is checked again every 10 ms.

Cancelling the fetch, or dropping it, ends the task at its next step.

- **One connection, one stream:** as for FTP (`DownloadOptions::one_connection`), until #77 measures whether more help. A cut or a pause resumes at its offset with one read from there.
- **The resume check:** the file's time is a strong `Last-Modified` once it is a second old, so a resume is checked against it ([changed files](#changed-files-45)). A file whose time changed starts over.

### Host keys

- **The store:** `<data folder>/known_hosts` (`Paths::known_hosts`), V2's own, never `~/.ssh/known_hosts`. It is unhashed, one OpenSSH line per host (`[host]:port key-type base64-key`), and compares the whole key. A host is shown by its key's SHA-256 fingerprint (`SHA256:…`, as OpenSSH prints it). Writes go to a temporary file, then a rename.
- **Unknown key:** refused before the login. The download fails at once, permanently, and `TransferError.hostKey` carries the host, port and fingerprint for the user to check. The transport remembers that key.
- **Trusting it:** `DirectTransfer::trust_host_key(host, port, fingerprint)` pins the key the transport saw with that fingerprint, and only that one: a fingerprint it didn't see is `NotFound`. The caller then retries the download. The app's `trust_host_key` command (#58) calls it through `Manager::trust_host_key`, and the window's host-key question (#60) shows the fingerprint and calls it.
- **Changed key:** always refused, before the login. `hostKey.pinned` holds the pinned key's fingerprint. Such a key is never offered for trust, and `trust_host_key` never replaces a pinned key (`InvalidState`).
- **Host certificates** are refused: V2 trusts no SSH certificate authority.

### Failures

| What happened | Kind | Retried |
|---|---|---|
| The host key is unknown, or changed | Permanent, with `hostKey` | No |
| The login was refused, or there's no user name | `AccessDenied` | No |
| The private key can't be opened (a wrong passphrase, for example) | `AccessDenied` | No |
| No such file | `NotFound` | No |
| The connection was refused, or broke part way | `Connect`, `Interrupted` | Yes, from the first missing byte |

## Metalink (#54)

A Metalink document lists files with their sources, hashes and piece hashes: Metalink 4 (RFC 5854, `.meta4`) or Metalink 3 (metalinker.org 3.0, `.metalink`). libcurl dropped Metalink in 7.78.0, so V2 reads it itself. The manager does the reading, in `velocity-core` (`metalink.rs`); the engine only downloads the files, with the hashes it already checks (#46). Metalink is off until the user turns it on, as V1's `advanced.enable_metalink` (ADR-0005); the setting is stored and shown by #30.

### Reading a document

- **The parser** is quick-xml (PO decision on #54), read as a stream. Its limits are in [SECURITY.md](SECURITY.md#metalink-54).
- **Per file:** its name, which may hold subfolders (`disc/image.iso`); its size; the strongest whole-file hash of MD5, SHA-1, SHA-224, SHA-256, SHA-384 and SHA-512; the strongest piece list, which must fit the size; and its sources.
- **Sources:** `http`, `https`, `ftp` and `ftps` URLs without a login, best first. Metalink 4's lower `priority` comes first and Metalink 3's higher `preference`; unranked ones come last, in document order. A duplicate counts once. SFTP sources are dropped (a document can't supply a login or confirm a host key), as are torrents (`metaurl`, a Metalink 3 `bittorrent` resource; a non-goal of #54).
- **A file with no usable source** is skipped and counted (`Metalink::skipped`), and a document with none at all is refused.
- **Not supported, as in V1:** Metalink/HTTP (RFC 6249), whose `Link` and `Digest` headers aria2 never read.

### Adding its files

`Manager::add_metalink` adds the selected files, all of them unless a selection names some, one download each with kind `metalink`:

- **The folder:** each file goes into its subfolders inside the chosen folder (PO decision on #54: kept, as V1 did). They're created one level at a time. A folder already there is used, but a file, a link or (Windows) any reparse point in the way is refused, never followed.
- **The download:** its sources the engine can fetch, best first, in `TransferSpec::urls`; its hash in `checksum` and its piece list in `pieces`; its name in `fileName`. No login, headers or referrer: a document's sources are other servers.
- **All or nothing:** a file with no source the engine supports refuses the lot before anything is added. A file the engine refuses after others were added takes those back (they're cancelled), and the refusal is the error.
- **A local file:** `metalink::read_file` reads a `.metalink` or `.meta4` file the user chose, as V1's `validate_local_metadata_file` did: an absolute path to a plain file, never a link. The file chooser and its Tauri command are #58's and #60's.

### Following a link

As aria2's `follow-metalink` did for V1: while Metalink is on, a completed HTTP or FTP download that is a Metalink file adds every file it lists, into its own folder.

- **How it's known:** its name ends `.meta4` or `.metalink`, or the server called it `application/metalink4+xml` or `application/metalink+xml` (`TransferSnapshot::contentType`, contract 11).
- **The Metalink file stays** on disk as a completed download.
- **One that can't be used** fails, with the reason: "Velocity downloaded this Metalink file but couldn't add its files: …". The file is kept.
- **Never twice:** a download that came from a Metalink is never followed, so one document can't chain to another.

## Mirrors (#55)

A download can have several sources: its URL and its mirrors, such as a Metalink file's (#54), in `TransferSpec::urls`, best first. Several are used at once, as V1's aria2 did (`uri-selector=feedback`; PO decision on #55). `download::download_from` takes them; `crates/velocity-transfer/src/sources.rs` keeps each one's health. A download with one URL takes exactly the path it took before.

### Which sources may share a file

Bytes from two sources are mixed only when they are proven to be the same file. A mirror's first answer to a range decides; until then it serves nothing (`sources::admission`, checked in the request's header callback before a byte is written):

- **Always:** the same total and the same `Content-Encoding` as the download's first answer. Another total or encoding disables the mirror for this download.
- **With trusted piece hashes:** that is enough. Every piece is checked as it is covered, so a mirror that sends other bytes is caught (below).
- **Without them:** it must also give the same strong validator (an entity tag, or a strong date) as the first answer. A mirror that doesn't is *unproven*: it serves no range, and is kept only as a fallback.
- **After that,** each mirror is held to its own first answer, as the first source is (#45), with its own `If-Range`.
- **Only HTTP and HTTPS** sources share. FTP and SFTP run on one connection (#51, #53), so they are only ever a fallback; a download whose first source is FTP or SFTP uses one source at a time.

### Who gets the next range

For each new request the pool picks a usable source that isn't cooling down and has room (at most the download's connections each):

1. One never asked yet goes first, once, so its speed gets measured.
2. Otherwise the one with the most speed to spare: its measured speed (bytes over the time its requests were busy) divided by its open requests plus one, halved for each failure. A source tried but not yet measured counts as fast as the fastest measured one. Ties go to the earlier source.

So two equal mirrors share a file about evenly, and one twice as fast takes about two thirds of the requests. With mirrors and piece hashes, the probe and every lease end on a piece boundary, so each piece comes from one source.

### When a source fails

- **A mirror's failure is the mirror's,** never the download's: a refused or reset connection, an error status, a cut-off or short answer, a `200` for a range, or an answer whose file changed. Its range is leased again, at once, from another source.
- **A transient failure** cools the source down for 2 s, doubled for each further one (`sources::COOLDOWN`); the third disables it for the download (`MAX_FAILURES`). A permanent one, such as a `404`, disables it at once. Both are starting values for #77 to measure.
- **A bad piece** is blamed on the source that sent it (`Sources::blame`): its standing halves and it cools down, and a second bad piece disables it. The piece is fetched again, from the healthiest source. When a piece came from several sources, which can only happen when a cut-off lease's rest went elsewhere, each gets an ordinary failure instead.
- **The first source** failing its probe, or every proven source failing, hands the download to the next source that isn't disabled. The download then starts over from it, since nothing proved its bytes the same, and the others must prove themselves against it again (`Sources::make_lead`).
- **None left:** the download fails with "Every source of the download failed; the last: …", and that failure's class decides whether the engine retries it.

### What a mirror is sent

The user agent and the referrer go to every source. The login and custom headers go only to the first URL's origin (#40), so a mirror elsewhere never sees them ([SECURITY.md](SECURITY.md#mirrors-55)). The engine checks every URL in the spec as it checks the first: one it may not fetch refuses the download.

### What the report says

`DownloadReport::sources` gives each source's URL, state (usable, unproven or disabled, with why), the bytes it served and its failures. Checkpoints name the source the download led with; a resume leads with that one again.

## Later segments: the range scheduler (#42)

After a probe that gave a total, the rest of the file is fetched with leases from the range scheduler ([`scheduler.rs`](../crates/velocity-transfer/src/scheduler.rs), report section 5.2). The downloader's thread owns it and makes every change; no lock is held across I/O.

### Leases

A lease owns `[cursor, end)` of the file alone; `[start, cursor)` is already written. Its id is never reused within a download.

| State | Meaning |
|---|---|
| Assigned | The request `Range: bytes=start-(end−1)` was made; no answer yet |
| InFlight | The answer was checked and accepted in the header callback (`check_segment`) |
| Completed | `cursor == end` |
| Failed | It ended early; the rest of its range is unassigned again |
| Revoked | It was cancelled; the rest of its range is unassigned again |

- **Assignment:** a free connection gets the front of the unassigned bytes, an equal share per free connection. The share is rounded up to `align` and is at most `max_lease`. Sharing spreads a file over the connections without splits.
- **Acceptance:** bytes are accepted only from the lease that is in flight, at its cursor, and only up to its *current* end. Bytes for a lease that isn't live (ended, revoked, unknown), at another offset, or past its end are dropped and counted as wasted. They enter coverage only once the write to the part file has returned.
- **Split (work-stealing):** a free connection with nothing unassigned takes half of the live lease with the largest remainder. The remainder must be at least `2 × min_split`, and the midpoint is rounded down to `align`.
  - The old lease shrinks to end at the midpoint.
  - Its request is told to stop there: `FetchLimit::stop_at` in the libcurl layer delivers bytes up to the limit, then stops the transfer, and reports that as a completed fetch, not an error.
  - Over HTTP/1.1 its connection closes. Over HTTP/2 only the stream is reset, and the connection is kept (tested in #47).
- **Retries:** a lease that ends before its end fails, and the rest of its range is handed out again under a new id. A late callback from the old request carries the old id and is refused.

| Size | Value | Where it comes from |
|---|---|---|
| `max_lease` | 32 MiB | Report section 5.2, a hypothesis for #47 to tune |
| `min_split` | 1 MiB (a remainder of at least 2 MiB splits) | A starting value; the report names no number |
| `align` | 64 KiB | Report section 5.2 |

### Answers that aren't what was asked

Each answer is checked in the header callback (`check_segment`):

- **Ranges lost:** a `200` is aborted before its body is written. When the live leases end, the rest of the file comes as one stream that writes only the missing bytes, so the file is never mixed.
- **Anomaly:** an answer that contradicts its request, such as a wrong or malformed `Content-Range`, a length that disagrees with it, or a multipart body. It also covers:
  - a body cut off before its range ended;
  - a chunked body that stopped short of its `Content-Range`;
  - a body that ran past its range.

  The first anomaly is tolerated, and the lease's rest is leased again. At the **second** on the same download (`MAX_ANOMALIES`), no more leases are handed out, and the rest comes as one stream that writes only the missing bytes (`RangesLost`).
- **Short answers:** a `206` with fewer bytes than asked is honest (RFC 9110 §14.2). Its lease fails, and only the remainder is leased again.
- **Failure:** an error status, or an answer about another file ([changed files](#changed-files-45)). An error status worth retrying frees the connection for a wait, and the rest of the lease is leased again after it ([retries](#retries-waits-and-stalls-48)).

### Invariants

Checked after every change in debug builds (`Scheduler::check`), and by the property tests:

- **I1 Partition:** the coverage C, the live leases' `[cursor, end)` and the unassigned bytes U are disjoint and make up `[0, T)`.
- **I2 Exclusivity:** no two live leases overlap, and none overlaps C.
- **I3 Acceptance:** a byte at offset p is written only by the live lease whose cursor is p.
- **I4 Monotonic coverage:** C only grows within an epoch; `new_epoch` starts again from an empty C.
- **I5 Durability:** the durable coverage D is part of C and was synced before it was recorded (`synced`). Saving D in a checkpoint is #43.
- **I6 Completion:** C covers the file and no lease is live (`is_complete`).
- **I7 Conservation:** bytes received = growth of C + wasted bytes.

## Accounting

Every received byte is either written into the part file once, or counted in `wasted_bytes`. That includes bytes past a lease's end, bytes for a lease that isn't live, bytes the single stream sends again, bytes already covered, and the bytes of a piece that failed its hash and was fetched again ([integrity](#piece-hashes)). Progress counts written bytes only. A server that ignores ranges costs exactly one GET with no wasted bytes, which a fixture test checks.

## Changed files (#45)

Every answer after the first must be about the file the first answer described (report section 5.3, [`validator.rs`](../crates/velocity-transfer/src/validator.rs)). Validators belong to the file, not to its URL, so an answer that a redirect brought from elsewhere is held to the same.

### The validator

| The first answer gives | Sent as `If-Range` on every later range request |
|---|---|
| A strong ETag, such as `"v1"` | The ETag |
| A weak ETag, such as `W/"v1"`, with or without a Last-Modified | Nothing. RFC 9110 §13.1.5 forbids a weak tag in `If-Range`, and a date while an entity tag exists |
| No ETag, and a Last-Modified at least 1 s before the answer's own `Date` | The date. 1 s is RFC 9110 §8.8.2.2's rule for a client (`STRONG_DATE_MARGIN`); the report keeps a larger margin as a hypothesis to test |
| Anything else, a malformed ETag included | Nothing. Only the total is compared, and a resume follows [ADR-0010](decisions/0010-resume-without-validator.md) |

The report's fixture table says a weak ETag still allows a strong Last-Modified. The RFC's text doesn't, and the engine follows the RFC.

### What every later answer is checked for

In the header callback, before any byte is written, an answer is about another file if it has:

- **another ETag,** compared as sent, so a weak tag never equals a strong one, or **no ETag** where the first answer had one (the report's conservative rule);
- **another strong date,** or none;
- **another total,** or a `416` for a range inside the old total;
- **another `Content-Encoding`,** which is its own case below.

A `200` to a range that carried `If-Range` is the new file when its validators differ (RFC 9110 §13.1.5), and ranges lost when they match. The same checks apply to the stream that fills the gaps after ranges are lost, and to a probe asked for again after it was cut off, whose bytes so far are the earlier answer's. That retry must also give the same length.

### What a change does

- **Another file:** every request stops, the part file is emptied, and the download starts over once from a new probe, which gives the new validator. Old and new bytes are never mixed. A resume whose file changed since its checkpoint does the same.
- **Another encoding:** ranges count encoded bytes, so the download starts over once as one stream (`Mode::SingleStream`).
- **A second change** after the start-over ends the download: "The file changed on the server during download, twice: …". It is Transient and retryable, so the user can retry it, but the engine doesn't restart it on its own. The part file is deleted.

The start-over uses a new scheduler and a new probe, not `Scheduler::new_epoch`, because the new file's total and validator come from the new probe.

### Content encoding

A server may apply an encoding nobody asked for, such as a `.tar.gz` served with `Content-Encoding: gzip`. The engine stores the bytes exactly as sent and never decodes them (PO decision on #45, replacing #41's refusal). Ranges work while every answer has the same encoding, which the checkpoint records (`contentEncoding`) so that a resume can check it. The rare cost: a file that a server compressed on the fly is saved compressed.

### Expired links

A `403` on any answer after the first fails with "The download link expired (403). Get a fresh link from the browser." (PO decision on #45: a message, with no new failure kind). A `403` on the first answer keeps "Access was denied by the server (403). The link may have expired.", as it may be a real refusal.

### What can't be detected

A server that changes the file but keeps its ETag, its date and its length looks unchanged. Only a checksum catches it ([integrity](#integrity-46)). Without any validator, only the total is compared ([ADR-0010](decisions/0010-resume-without-validator.md)).

## Retries, waits and stalls (#48)

A failure is classified first ([`failure.rs`](../crates/velocity-transfer/src/failure.rs)), and the retry policy then decides from its class alone ([`retry.rs`](../crates/velocity-transfer/src/retry.rs)). The messages use V1's words where V1 had them.

| Class | Examples | What happens |
|---|---|---|
| Transient | No answer, or a connection broken during the request; a name that doesn't resolve; a refused connection; the 30 s connect timeout; a failed TLS handshake; `408`; other `5xx`; a stall | Retried after a wait |
| Throttled | `429`, `503` | Retried after at least what `Retry-After` asks for (seconds or an HTTP date), up to 10 minutes. Without `Retry-After`, at least 60 s, as in V1. The download's connections are halved, down to one |
| Permanent | `401`, `403`, `404`, `410`, other `4xx`; an untrusted certificate; too many redirects; a protocol outside the allowlist; an answer that contradicts itself; a full disk, or a folder V2 may no longer write to | Never retried. A full disk or a refused write keeps the part file and a checkpoint of what reached the disk, so the user's retry, once it is fixed, resumes (#74) |

### Inside a download

- **What is retried:** the probe, each range segment, each sequential range and the single stream.
- **A failed segment** frees its connection for the wait. A new lease then takes the rest, under a new id. The other connections go on in the meantime.
- **The wait:** it doubles from 1 s up to 60 s, with full jitter, which means a random wait between zero and that value (`fastrand`).
- **Bounded:** a download gives up after 5 failures in a row with no new byte in between. A retry that brings new bytes starts the count again, so every retry either makes progress or brings the end closer.
- **The probe:** a probe whose answer gave the file's total and was then cut off isn't retried. Its bytes are kept, and the range scheduler fetches the rest.
- **A connection cut mid-body** still counts as an anomaly (#41), and two anomalies switch the download to one stream. A stall and a connection that fails before any answer count as failures, not anomalies.

### Stalls

- **The rule:** a stall is time with no byte from the network. It is never a speed limit, so a slow but steady transfer is never reset.
- **Stalled after 15 s:** with no byte, the download shows Stalled. With several connections, it shows Stalled only when none of them brings bytes.
- **Reset after 30 s:** with no byte, the request is cancelled and retried like any failure. These replace the prototype's fixed 60 s limit.
- **Disk waits don't count:** each segment's forwarder starts its clock only while it waits for libcurl, and the single-stream reader starts it after each write. Time spent waiting for a slow disk is never taken for a stall ([engine-mode-costs.md](research/engine-mode-costs.md) measured disk stalls of up to 57 s).

### The whole download

A download that still fails in a way worth retrying is started again by the engine (V1's `app_retries`):
- at most 2 times;
- after 5 s and then 10 s, or 60 s and then 120 s when the server was throttling, never more than 15 minutes;
- from its last checkpoint when it has one (its part file was kept), otherwise from the start.

While it waits, the snapshot shows Retrying, with the failure's message and "Retrying automatically (1 of 2).". Once bytes come again, the error is cleared. A cancel during the wait removes the kept part file. A permanent failure is never started again. Retrying by hand is the manager's, and never a second automatic retry ([ENGINE_CONTRACT.md](ENGINE_CONTRACT.md#retrying-removing-and-clearing-58)).

### The values

All of them are starting values to tune by measurement, not requirements (PO decision on #48). `RetryPolicy::DEFAULT` holds them, and the connect timeout is `LoopOptions::connect_timeout`. Settings will make them the user's (#30). The engine in the app's process takes another policy with `InProcessEngine::start_with`. The child process uses the defaults, because the contract doesn't carry a policy.

### Statuses

`Control::with_status` reports Retrying while a download waits, with when it tries again (#59), Stalled while no byte comes, and Downloading when bytes come again, each change once. With several connections, Retrying shows only when every connection waits, and its time is the first to try again. A restart of the whole download reports its own time, never the last request's. The engine puts them in the snapshot. An engine that hangs without exiting is the supervisor's ([ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md#supervisor)).

## Speed limits and back-pressure (#49)

### Speed limits

- **Two limits per transfer:** each transfer has its own limit (`TransferSpec::limits`, changed by `set_limits`), and every transfer also draws from the engine's global limit (`set_global_limits`). The lower of the two applies.
- **Sharing:** whatever the global allowance a transfer doesn't use goes to the others. With a global 4 MiB/s and one download held to 1 MiB/s, another download gets the remaining 3 MiB/s.
- **Token buckets** ([`rate.rs`](../crates/velocity-curl/src/rate.rs)): each limit is a token bucket, `RateLimit`. It refills at its rate, and keeps at most 250 ms of unused allowance (a starting value).
  - Each request of a download draws from its download's bucket and the global one.
  - When a bucket has no tokens, libcurl's write callback pauses the transfer (`CURL_WRITEFUNC_PAUSE`), and libcurl keeps the bytes.
  - The libcurl loop thread checks every 10 ms, and resumes the transfer once all its buckets have tokens again.
  - A write is taken whole while a bucket has any tokens. So a bucket owes at most one write (16 KiB), and later writes wait until it is paid back.
- **Fair turns:** resuming a transfer hands it libcurl's kept bytes at once. So the order of resuming turns each round, and no transfer always comes last to a shared bucket.
- **Live changes:** a limit's rate changes at once. The next write sees it, so a change applies within one loop round (10 ms).
- **Why not libcurl's own cap:** `CURLOPT_MAX_RECV_SPEED_LARGE` is "not an exact science" by its own documentation, and its documentation doesn't say whether it can change during a transfer (report open question 3).
- **The smallest limit is 1 KiB/s,** V1's unit. A lower one is refused with `INVALID_INPUT`. A low limit is bursty, as data arrives in writes of up to 16 KiB, but its average holds.
- **HTTP/2:** a paused stream may still receive up to one stream window of data, which libcurl keeps (curl_easy_pause documentation). The HTTP/2 policy is in [Connections](#connections-47).
- **After a restart:** the supervisor gives every new engine the global limit the old one had. A transfer's own limit is in its spec, which the app passes again when it resumes the transfer.

**Measured** on the local test server (Windows 11, `tests/limits.rs`):

| Case | Expected | Measured |
|---|---|---|
| Global 4 MiB/s, 3 downloads, over 10 s, 8 runs | 4.00 MiB/s | 3.997 to 4.003 MiB/s |
| The same over 30 s, by hand | 4.00 MiB/s | 4.000 MiB/s |
| 0.5 MiB/s raised to 4 MiB/s, the 2 s after the change, 5 runs | 4.00 MiB/s | 3.975 to 3.983 MiB/s |
| 1 MiB/s download under a global 4 MiB/s, 5 runs | 1.00 MiB/s | 1.022 to 1.031 MiB/s |
| The other download, 5 runs | 3.00 MiB/s | 2.972 to 2.978 MiB/s |

### Back-pressure

A slow disk slows the network instead of growing memory. This was built in #37 and #38 and is now measured:

- **Per request:** each request holds at most `LoopOptions::buffer_bytes_per_fetch` (1 MiB) for the writer. When that buffer is full, libcurl's write callback pauses the transfer, and the reader resumes it once half is free.
- **The queue:** at most 2 events per connection wait between the requests and the writer.
- **Measured:** `DownloadReport::peak_buffered_bytes` reports the most one request held at a time. The slow-writer test stalls the writer for 2 s, and checks that each buffer filled and never went past its cap by more than one 16 KiB write.
- **The kernel's share:** the sockets' own buffers belong to the kernel. They hold more, depending on the system, and on Windows they kept growing during a pause (#147, #150). Only Velocity's own buffers are checked exactly.

## Connections (#47)

How many connections a download uses, and how they are shared. The PO decisions are on #47.

### What a download may use

A download's connections are the tightest of these, from the outside in:

| Limit | Where it comes from | Shown as |
|---|---|---|
| What it asked for | `TransferSpec::max_connections`, or `set_connections` later, at least 1 and at most the per-host cap | `requested` |
| Its share of the per-host cap and the shared budget | The engine, max-min fairly between the running transfers ([`budget.rs`](../crates/velocity-transfer/src/budget.rs)) | `perHostCap`, `sharedBudget` |
| A throttling server | A `429` or `503` halves the connections, down to one (#48) | `serverThrottling` |
| What helps | The adaptive ramp ([`adaptive.rs`](../crates/velocity-transfer/src/adaptive.rs)) | `rampingUp`, `noGain` |
| What is left to split | No remainder of at least 2 × `min_split` | `fileSize` |

Snapshots carry the connections in use now (`Progress::connections`) and `connectionDetail`: `requested`, `allowed` and the tightest limit, `limitedBy` ([CONTRACTS.md](CONTRACTS.md)).

### Live changes

- **`set_connections`** changes the count while the transfer runs, without a restart: no new probe, and the part file and its coverage stay as they are.
- **More** take new leases at the next step of the download, splitting a live lease when nothing is unassigned.
- **Fewer** stop the newest requests. Each stops at its next write (`stop_at(0)`), the bytes it already delivered are kept, and the rest of its lease is leased again. The fixture checks that its changes waste no byte.
- **A paused transfer** keeps the count for when it resumes.

### Budgets and fairness

- **Per host:** at most 8 connections to one host name and port, shared by every transfer to it. libcurl's `CURLMOPT_MAX_HOST_CONNECTIONS` is set to the same 8, as the backstop: a fetch past it waits for a free connection.
- **Shared budget:** at most 48 connections across all transfers.
- **Max-min fair:** the engine shares both again whenever a transfer starts, ends or changes its count. Connections go one per transfer per round, so no transfer gets more while another that wants more has fewer; a small download's unused share goes to the others.
- **One each:** every running transfer keeps one connection, even past the caps, as a download makes no progress without one. How many transfers run at once is the queue's job (#50).
- **The budget always applies,** to a count set by hand too (PO decision on #47; V1 let a hand-set count ignore it).

### Adaptive connections

The asked-for count is the most a download may use, not a must (PO decision on #47). Every download in the engine starts with one connection, the probe, and ramps up:

- **Windows:** the ramp measures the body bytes received over a window of max(2 s, 4 × RTT). RTT is the shortest wait seen between a request and its answer.
- **Add while it helps:** after a window in which the last connection added raised the speed by at least 10%, it adds another.
- **A small gain** (2% to 10%) keeps the last one and stops.
- **No gain:** after 2 windows below 2%, it drops the last one and stops.
- **No decision** while fewer requests run than the ramp wants, at a file's tail for example: that speed says nothing about the count.
- **A lower ceiling** (the budget, the user, a throttling server) applies at once. **A higher one** starts the ramp again.
- **Example:** asked for 8, from a server that gives one client no more than 3 connections' worth, a download keeps 3, and shows `noGain`.

`DownloadOptions::adaptive` is `None` for a fixed count, which the CLI uses to compare fixed plans.

### HTTP/2

- **Multiplexing first:** requests to one host wait for the connection being set up (`CURLOPT_PIPEWAIT`), and share it as streams if it speaks HTTP/2.
- **Then connections of their own, once:** when the ramp finds that more streams stop helping, the download tries requests on connections of their own (`CURLOPT_FRESH_CONNECT` and `CURLOPT_FORBID_REUSE`), ramping again from one.
- **The faster way is kept:** connections of their own stay only if their best speed beats the best multiplexed one by the ramp's 10%. Otherwise the download goes back to the multiplexed count. Changing the way stops every request first, so each way is measured on its own.
- **`DownloadReport::own_connections`** says which way was kept.
- **A split** resets only its stream; the connection stays.

### The values

All are starting values to tune by measurement (#77); the user can change them ([SETTINGS.md](SETTINGS.md#engine), #30).

| Value | Starting value | Where it comes from |
|---|---|---|
| Per-host cap | The performance profile's: 8 for Balanced, 2 to 16 for the others ([SETTINGS.md](SETTINGS.md#performance)) | Report section 5.5 (8, at most 16); PO decisions on #47 and #57. `set_engine_limits` changes it while transfers run, and libcurl's own per-host cap (`CURLMOPT_MAX_HOST_CONNECTIONS`) with it: with many small downloads at once, each keeping one connection, that is what bounds a host |
| Smallest piece (`Sizes::min_split`) | The profile's: 1 MiB for Balanced | PO decision on #57 |
| Shared budget (`CONNECTION_BUDGET`) | 48 | V1's `connection_budget`; PO decision on #47 |
| Window | max(2 s, 4 × RTT) | Report section 5.5 |
| Gain to add another | 10% | Report section 5.5 |
| Flat gain, and windows before stepping back | below 2%, 2 windows | Report section 5.5 |

**Measured** on the local test servers (Windows 11). These check behaviour, not speed: no speed claim is made until #77 (PO decision on #47):

| Fixture | Expected | Observed |
|---|---|---|
| 4 downloads from one host, 8 asked for each, 2 MiB/s per connection | 2 each (`perHostCap`); Jain's index ≥ 0.9 | 2 each; Jain's index 1.000 in both engine modes |
| 1 MiB/s per connection, 8 asked for | Ramps up to 8 | 8 |
| 1 MiB/s per connection, 3 MiB/s per client, 8 asked for, 5 runs | Stops below the cap | Kept 3, at most 4 at once, every run |
| HTTP/2, 1 MiB/s per TCP connection, 5 runs | Connections of their own | 9 or 10 connections, 6 to 8 at once |
| HTTP/2, 1 MiB/s per stream, 5 runs | One multiplexed connection | 1 connection, 8 streams |
| 40 small files from one host | Reuse | 1 connection one by one; at most 8 at once |

The "at once" counts above are the server's. Since #154, the ramp fixtures check the engine's own count instead, `DownloadReport::peak_requests`.

## Persistence and recovery (#43)

A download survives a pause, an app restart, an engine crash and a power loss without a corrupt or falsely finished file. The rules are in the [persistence rule](../.claude/rules/persistence.md); the PO decisions are on #43.

### Checkpoints

- **When:** every 2 s or 16 MiB, whichever comes first (`Cadence::DEFAULT`, the report's starting values). Also when a download is paused. The probe takes them too once its answer has given the file's total, because the probe can be the whole download.
- **How, in this order:**
  1. the part file is synced (`sync_data`);
  2. the scheduler marks what is covered as durable (D);
  3. the checkpoint is sent.

  A byte is in D only once it is on disk (invariant I5).
- **What it holds** (`Checkpoint`, [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md)):
  - a format version (`CHECKPOINT_FORMAT`);
  - the source without its login, query and fragment, so no token;
  - the total;
  - what the first answer said about the file: its ETag, its Last-Modified only when that was the strong validator, and its `Content-Encoding` ([changed files](#changed-files-45));
  - D as half-open byte ranges.

  Leases are never saved.
- **Where it goes:** the engine sends it as a `checkpoint` event over its pipe. The app saves it in its database (the `checkpoints` table, [DATA_AND_MIGRATIONS.md](DATA_AND_MIGRATIONS.md)); only the app touches the database (PO decision). The manager saves it, drops it once the download completes or is removed, and resumes from it when the app starts again or the engine was lost ([ENGINE_CONTRACT.md](ENGINE_CONTRACT.md#saving-and-starting-again-58)).
- **Only ranges resume.** A single stream (a server without ranges) and a download without a total (sequential ranges) take no checkpoint, because nothing could resume them. Pausing them keeps the part file, and resuming starts over.

### Resuming

The app hands back what it saved, in `TransferSpec::resume`:

| `resume` | The engine |
|---|---|
| `checkpoint` | Trusts the checkpoint only if it fits: this format, this download's source, and ranges that are sorted, disjoint, non-empty and inside the total. If it fits, the part file is reopened (never through a link), only D is trusted, and only the rest is fetched, with the recorded validator in `If-Range`. Another validator, total or encoding, or a `416` inside the old total, means the file on the server changed: the old bytes are discarded and the download [starts over once](#what-a-change-does). If it doesn't fit, or the part file is gone, the download starts over |
| `discardPartFile` | The part file is the download's own but has no usable checkpoint: it is deleted and the download starts over (PO decision). A link, or anything that isn't a plain file, is refused and never touched |
| none | A new download. An existing part file is never reused: the download fails |

`StoredDownload::resume` in the database layer makes the choice from a record and its saved checkpoint. It picks the checkpoint if it's known and of this record's source, `discardPartFile` otherwise, and nothing for a finished record. A record this build can't route is never resumed.

### Pausing, stopping and crashing

- **Pause:** the requests stop, a last checkpoint is taken, and the part file stays. **Resume** starts a new worker from that checkpoint. Cancelling a paused transfer deletes its part file.
- **When the app goes away,** or closes the engine's pipe, the engine pauses every transfer and keeps their part files, so the app can resume them when it starts again. It no longer cancels them, as #39 did.
- **A failure:** one worth retrying, after a checkpoint was taken, keeps the part file, and the engine starts the download again from that checkpoint ([the whole download](#the-whole-download)). Any other failure deletes it.
- **A crash** (`kill -9`, a power loss) leaves the part file and the last checkpoint the app saved. Nothing that looks finished exists until the rename.

### Finishing

The file is renamed into place only when coverage is complete and, with a checksum, the hash matches ([the completion gate](#the-completion-gate)):

1. the part file is sized, checked and synced;
2. it is renamed **without ever replacing** an existing file. On Windows that's `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`, and with `MOVEFILE_WRITE_THROUGH`. On Linux it's `renameat2(RENAME_NOREPLACE)`, then a sync of the folder, so the new name survives a power loss.

A file that appeared at the destination during the download is kept, and the download fails with `FileExists`. On a Linux file system without `RENAME_NOREPLACE`, a hard link does the same job. The rename, unlike #37's hard link, also works on FAT32 and exFAT drives.

### Part files

[`files.rs`](../crates/velocity-transfer/src/files.rs) holds the only operations on part files:

- **Creating one:** exclusively, never over an existing name and never through a link. It's marked sparse on Windows (`FSCTL_SET_SPARSE`), so NTFS doesn't zero-fill the gap in front of a segment (see below).
- **Reopening one:** without following a link, checked again on the open handle.
- **Deleting one:** only if it's a plain file, never a link.
- **The no-replace rename:** described above.

The calls go through `windows-sys` and `libc` (PO decision), each `unsafe` call with its `SAFETY` note. `DownloadReport::sparse` says whether this run's part file was made sparse. A file system without sparse files, such as FAT or exFAT, still works, only slower.

## Integrity (#46)

A download is Completed only once its bytes are proven as far as the trusted hashes it has allow (report section 5.3, [`integrity.rs`](../crates/velocity-transfer/src/integrity.rs)). Hashing never runs on the network or downloader thread.

### The completion gate

In this order:

1. coverage is `[0, T)`;
2. the part file's length is T;
3. every piece matches its trusted hash, when there are piece hashes, and the whole-file hash matches the checksum, when there is one. The status is Verifying while the end of the whole-file hash is computed;
4. the part file is synced;
5. it is renamed without replacing an existing file;
6. on Unix, the folder is synced.

### The hash frontier

Hashes don't compose: the digests of separate segments can't be combined into the file's. So the frontier, on a thread of its own, hashes the covered prefix from byte 0. The part file tells it, after every write, how far the bytes from 0 reach; it reads the new ones back and hashes them. At completion it hashes whatever is left. Its state isn't saved, so after a crash or a resume it starts again from byte 0, reading back what is on disk.

- **The worst case** is one full sequential read at completion: the first segment finishes last, as when the range after the probe is the slowest.
- **A checksum is never skipped:** without a running frontier, the whole file is hashed at completion.
- **Memory:** the frontier holds one 1 MiB buffer and one number, how far it may hash; the writer never waits for it.
- **Cancel and pause while it waits:** completion waits for the frontier and for the last piece verdicts, checking every 100 ms (`CANCEL_CHECK`). A cancel ends the download and removes the part file; a pause keeps it, and a resume hashes again from byte 0. Either thread stops after its current 1 MiB read.

### Piece hashes

`TransferSpec::pieces` holds trusted hashes of fixed-size pieces, such as a Metalink file's `<pieces>` (#54 fills it; PO decision on #46). Hashes Velocity computes itself would only catch disk errors, never wrong server content, so only trusted ones are used.

- **Checked once covered:** each piece goes to the piece verifier, on a thread of its own, as soon as its last byte is written, in every mode. The last piece ends with the file, so it goes once the total is known.
- **Re-fetched alone:** once the file is covered, the engine waits for every verdict. A bad piece is taken out of coverage, its bytes from this attempt count as wasted, and the download fetches the missing ranges again with a new scheduler built from the coverage. The range scheduler isn't changed for this, so invariants I1 to I7 hold as before.
- **At most 2 re-fetches** per piece, 3 tries in all (PO decision on #46). A third bad copy fails the download: "The downloaded file's piece n of m doesn't match its trusted hash, even after fetching it again 2 times: the server sends bad data. …".
- **Without ranges** (a single stream, or ranges lost), a piece can't come alone, so the whole file is downloaded once more as one stream (PO decision on #46). A bad piece again fails it.
- **A list that doesn't fit** the file, with more or fewer pieces than its size needs, fails at once as Permanent.
- **With a checksum as well,** the frontier starts again from byte 0 after a piece is fetched again, because it may have hashed the bad bytes.
- **Mirrors:** a bad piece is blamed on the source that sent it, and fetched again from the healthiest one ([Mirrors](#mirrors-55)).

### A failed check

A wrong checksum, or a piece that stays bad, fails the download with a message and `DownloadError::IntegrityFailed`, as Transient and retryable:

- the part file is **kept** for inspection, and not renamed;
- the engine **doesn't start it again** on its own, as that would replace the part file;
- when checkpoints were sent, a last one follows that **trusts no byte**, so a resume from it downloads the whole file again instead of checking the same bytes a second time (report section 5.3: "A retry re-downloads once").

A piece list that doesn't fit the file fails the same way, but as Permanent: the list or the server is wrong.

### What was proven

`DownloadReport::verification` and the snapshot's `verification` say what the completed file's bytes were checked against ([ADR-0010](decisions/0010-resume-without-validator.md)):

| Verification | When |
|---|---|
| `verified` | A checksum or trusted piece hashes were given, and they matched |
| `notVerified` | No hash, and the download resumed from bytes on disk without a strong validator, so a file that changed at the same length could have mixed two versions |
| `sizeVerified` | Anything else complete: every byte arrived and the length matched |

"Verified" is shown only when a hash was checked. The manager saves the value (`downloads.verification`, #58).

### Read-back cost

Measured with [`examples/hash_readback.rs`](../crates/velocity-transfer/examples/hash_readback.rs) (the issue's risk; #46, 2026-10-23) on Windows 11 with an Intel Core i7-13620H (16 threads, 16 GB). A 1 GiB file is written in 1 MiB chunks to a part file, and the frontier follows. Medians of 3 runs, in seconds:

| Algorithm | Hash alone, from memory | Completion waits, segments in reverse (worst case) | Completion waits, segments in order |
|---|---|---|---|
| MD5 | 2.89 | 3.20 | 3.04 |
| SHA-1 | 0.91 | 1.18 | 1.16 |
| SHA-256 | 1.35 | 1.60 | 1.34 |
| SHA-512 | 3.30 | 3.40 | 3.52 |

What they show, and what they don't:

- **The hash itself is most of the cost.** Reading back the whole file adds 0.1 to 0.3 s per GiB here, because the file was just written and the read comes from the operating system's cache. A read from the disk itself would take longer; it isn't measured.
- **In order isn't faster here,** because the example writes from memory far faster than the hash runs, so the frontier is still behind when the last byte lands. A download slower than the hash, as from a network, keeps it nearly caught up; that isn't measured either.
- **SHA-1 and SHA-256 are faster than MD5 on this CPU,** which has SHA instructions that RustCrypto uses.
- **Single runs vary:** the slowest of the three was up to 1.4 times the median.

Run it with `cargo run --release -p velocity-transfer --example hash_readback -- [dir] [--mib N] [--runs R] [--algorithms md5,sha1,sha256,sha512]`. The numbers above were taken with `--config profile.release.debug-assertions=true`, which the examples needed until the HTTP/2 tests moved out of `velocity-transfer` (below); debug assertions don't touch the hashing code.

## Disk writes far into a new file (Windows)

Segments are written in parallel, so a later segment writes far past what is already in the part file. Measured with `crates/velocity-transfer/examples/high_offset_write.rs` (report open question 7; #42, 2026-10-23). It writes 1 MiB into a new file and syncs it, 3 runs each:

| Machine | Offset of the write | No preallocation | After `set_len` | At offset 0 |
|---|---|---|---|---|
| Windows 11 (build 26200), NTFS, the system temp folder | 1 GiB | 0.6, 67 and 35 s | 4.5, 61 and 32 s | 0.4–0.5 ms |
| | 4 GiB | 120, 158 and 92 s | 128, 122 and 82 s | 0.4–0.5 ms |
| WSL2 Ubuntu 24.04, ext4 | 1 GiB and 4 GiB | 0.5–1.0 ms | 0.5 ms | 0.5 ms |

- **What happens:** NTFS zero-fills the whole gap before the write returns, here at about 30 to 50 MB/s. `set_len` doesn't help, because it doesn't move NTFS's valid data length. ext4 leaves the gap empty.
- **The effect, before #43:** a multi-GB parallel download on Windows could stall for minutes on a later segment's first write. The occasional 10 to 18 s runs of the fixture tests on Windows were likely the same effect, on a 128 MiB file.
- **The fix (#43, PO decision on #42):** the part file is sparse on Windows, so the gap is never filled.

**After #43**, on the same Windows machine (2026-10-23, `--only sparse,at_zero`, 3 runs each), the 1 MiB write into a part file as the engine creates it:

| Offset of the write | Sparse part file: write | Write and sync | At offset 0: write and sync |
|---|---|---|---|
| 1 GiB | 0.4–0.6 ms | 1.8–2.2 ms | 1.8–4.7 ms |
| 4 GiB | 0.5–0.7 ms | 2.0–7.2 ms | 1.7–2.1 ms |
| 16 GiB | 0.5–0.7 ms | 1.7–10.6 ms | 1.7–1.9 ms |

The gap is no longer filled: a write far into a sparse part file costs what a write at its start does. On a file system without sparse files (FAT, exFAT), the zero-fill remains.

Run it with `cargo run --release -p velocity-transfer --example high_offset_write -- [dir] [--gib N] [--runs R] [--only no_prealloc,set_len,sparse,at_zero]`.

## Tests

- **Fixtures:** `crates/velocity-transfer/tests/download.rs` runs the real engine against the local test server ([`velocity-test-http`](../crates/velocity-test-http/src/lib.rs)), whose options make it misbehave on demand. There is one test per row of report section 5.1's table, each checking the file's bytes, the number of GETs and the wasted bytes. A further test slows one connection so that a split happens, and checks that the old request stops at the boundary.
- **Properties:** `crates/velocity-transfer/tests/scheduler.rs` runs 3,000 generated scenarios of a simulated transport through the scheduler, with `proptest`. The scenarios mix random requests, chunk sizes, splits, refused answers, duplicate and late callbacks, cancellations, syncs, crashes that keep only D, and new epochs. After every event it checks I1 to I7 and that every covered byte equals the source. At the end it checks that the finished file is the source. Breaking the scheduler on purpose is caught: a split that doesn't shrink the old lease, accepting bytes at any offset, and a failed lease that doesn't give its bytes back each fail it.
- **Persistence fixtures:** `tests/download.rs` also covers:
  - checkpoints of synced bytes, the probe's included;
  - a pause and a resume from the last checkpoint, fetching only what was missing;
  - a file that changed on the server, which starts over;
  - checkpoints that don't fit, which aren't trusted;
  - a missing or discarded part file;
  - a file created at the destination mid-download, never replaced;
  - the sparse part file.

  `tests/engine_contract.rs` checks pause, resume and cancel in both engine modes.
- **Crash trials (L4):** `tests/crash_recovery.rs` kills a real engine (`kill -9`, `taskkill /F`) at a random moment, then resumes in a new one from the last checkpoint the first sent. Every trial checks four things:
  - nothing looks finished before the rename;
  - the file equals the source;
  - what wasn't durable at the kill is at most one checkpoint interval's worth;
  - the bytes fetched twice are at most that plus what was in flight.

  The same file kills the app too (#74): the test binary starts itself again as an app process (the manager over a database file, with its own child engine), kills it at a random moment, waits for its orphaned engine to stop, then opens the same database in a new manager, which brings the download back from its stored checkpoint and must finish it with the source's bytes.

  CI runs 3 trials of each. The full 100 run by hand with `VELOCITY_CRASH_TRIALS=100 cargo test -p velocity-transfer --test crash_recovery -- --nocapture` (PO decision on #43), and `VELOCITY_CRASH_SEED` repeats a run.
- **A full disk and a refused write (#74):** `crates/velocity-transfer-h2c/tests/disk_faults.rs`, through the manager and the in-process engine. velocity-transfer's test-only `test-faults` feature makes the part file's writes fail after a chosen number of bytes, as a full disk (`StorageFull`) or a refused write (`PermissionDenied`) would; an optimised build refuses the feature, like `test-ca`. No real small drive is made (PO decision on #74). The fault strikes once inside the 4 MiB probe and once among the parallel segments, never on a chunk boundary, so the failing write is cut part-way, as an interrupted write is. Each case checks:
  - the download fails, as permanent, with "There is not enough disk space." or "Velocity isn't allowed to write to that location.";
  - the part file and a checkpoint of what reached the disk are kept;
  - once the fault is cleared, the user's retry fetches only what was missing, and the file equals the source.
- **Changed-object fixtures (#45):** `tests/changed.rs` checks every request's `Range` and `If-Range` against a server whose file changes after a set number of GETs:
  - a new strong ETag starts over once and ends as the new file;
  - a dropped ETag counts as a change;
  - a weak ETag is never sent, and a strong date is;
  - a second change fails as retryable, with no part file left;
  - a resume whose ETag changed starts over;
  - the gap stream after ranges were lost, and a retried probe, are checked too;
  - an unasked encoding is stored as sent, a changed one starts over as one stream, and a resume needs its checkpoint's encoding;
  - a `403` after the first answer says the link expired.
- **Retry fixtures:** `tests/retries.rs` covers #48's acceptance fixtures:
  - a `429` with `Retry-After: 5` gets no request before 5 s;
  - a reset storm backs off within its bounds and recovers, and one that doesn't end fails after 5 tries;
  - a 20 KiB/s server is never reset;
  - a frozen connection shows Stalled, then is reset and retried;
  - a name that doesn't resolve is transient;
  - a `404` is permanent with its message;
  - a cancel during a wait ends at once.

  `tests/engine_retries.rs` covers starting a whole download again, giving up after the restarts, never restarting a permanent failure, a cancel during the wait, and Stalled in the snapshot. `tests/supervisor.rs` freezes a real engine process and checks it is restarted.
- **Limit fixtures:** `tests/limits.rs` measures the bytes written over a window: a global 4 MiB/s across 3 downloads, a change taking effect within 2 s, and a download limit under a higher global one, each within ±10% over 10 s. `VELOCITY_LIMIT_SECONDS=30 cargo test -p velocity-transfer --test limits -- --nocapture` runs the 30 s window of #49 by hand (PO decision on #49). `tests/engine_contract.rs` checks limits in both engine modes, and `tests/supervisor.rs` checks that the global limit survives a restart.
- **Connection fixtures (#47):** `tests/connections.rs` changes a download's count live (up and down, with no byte wasted and no second probe), holds it to the engine's allowance, reports when the file is too small for more, and runs the adaptive ramp against a per-connection and a per-IP throttle. `tests/engine_contract.rs` checks `set_connections`, the per-host share with Jain's index, and connection reuse in both engine modes. Since #154, so that slow machines don't fail them:
  - `tests/connections.rs` runs its tests one at a time, and so do the three in `tests/engine_contract.rs` that measure speed. At once, on 2 vCPUs, they stretched the ramp's windows to almost 3 s;
  - the ramp fixtures use the engine's own policy, with its 2 s windows. With 1 s windows, noise passed for a 10% gain;
  - **still known:** about 1 run in 25 on 2 vCPUs, the per-IP fixture sees the ramp try a 5th connection when noise passes for a gain. The check stays at 4, a CI failure there is re-run, and the ramp's response to noise is for #77 (PO decision on #154);
  - the ramp fixtures check "never more than asked" with the engine's own count, `DownloadReport::peak_requests`. A server counts a stopped request for a moment longer, because it writes the request's tail until it sees the connection close;
  - the fairness fixture measures once every download is downloading on its 2 connections. A download's allowance alone is set before it connects.
- **Integrity fixtures (#46):** `tests/integrity.rs` runs against the test server:
  - a correct checksum completes a download whose segments land out of order, with Verifying shown;
  - a wrong one fails with the part file kept and nothing renamed, and a resume from its last checkpoint downloads every byte again;
  - checksums on a single stream and an empty file;
  - Verified, Size verified and Not verified, for fresh and resumed downloads with and without a strong ETag;
  - a piece the server sends bad once (`ServerOptions::corrupt_once`) is fetched again alone;
  - a piece that stays bad fails after 2 re-fetches;
  - without ranges, a bad piece starts the download over once;
  - a piece list that doesn't fit fails;
  - after a piece is fetched again, the whole-file hash still matches.

  The unit tests in `integrity.rs` check the six algorithms against their standard test vectors, a frontier fed in reverse order, and the piece verifier. `tests/engine_contract.rs` checks checksums and piece hashes in both engine modes.
- **HTTP/2 fixtures (#47):** `crates/velocity-transfer-h2c/tests/http2.rs` runs against the cleartext HTTP/2 server: #41's probe fixtures, #42's split that resets only its stream, and the multiplex policy against a server throttling each TCP connection and one throttling each stream. They need the `test-h2c` feature, which only test builds have ([TESTING.md](TESTING.md#fixture-servers-v2-needs)). They live in a test-only crate of their own (#46): a dev-dependency's features reach every example of its crate, so while they were in `velocity-transfer`, none of its examples could be built for release.
- **TLS fixtures (#69):** `crates/velocity-transfer-h2c/tests/tls.rs` downloads in segments over HTTPS and over HTTP/2 negotiated through ALPN, and checks that a refused certificate fails permanently with nothing on disk. It needs the `test-ca` feature, so it lives in the same test-only crate. What each TLS backend accepts and refuses is in [SECURITY.md](SECURITY.md#tls-verification-69).
- **FTP fixtures (#51):** the FTP server in [`ftp.rs`](../crates/velocity-test-http/src/ftp.rs) is V1's test server ported to the standard library. It serves passive FTP with `SIZE`, `MDTM` and `REST`, explicit and implicit FTPS on a `TestPki`, and switches for no `SIZE`, no `REST`, no `MDTM` and a cut transfer. It records the logins, `RETR`s, `REST` offsets and which connections were encrypted.
  - `crates/velocity-curl/tests/ftp.rs` checks the head built for each case in the table above, the login, a refused login, a cut, and FTPS: explicit, implicit, required from a plain server, and an untrusted certificate.
  - `crates/velocity-transfer/tests/ftp.rs` ports V1's tests: a whole file whose SHA-256 matches, a cut transfer resumed with `REST`, and a missing file. It adds a pause resumed from its checkpoint, a resume with and without `MDTM`, a file whose time changed, and a server without `REST`.
  - `tests/engine_contract.rs` downloads over FTP with a login in both engine modes, on one connection.
  - `crates/velocity-transfer-h2c/tests/ftps.rs` downloads over explicit and implicit FTPS on the test CA, and checks that requiring encryption from a plain server, and a refused certificate, fail at once without the login.
- **Unit tests:** the tests in `probe.rs`, `validator.rs`, `failure.rs`, `retry.rs`, `adaptive.rs`, `budget.rs`, the libcurl layer's `rate.rs`, `scheduler.rs`, `files.rs` and the libcurl layer's `tests/curl_loop.rs` (`FetchLimit`, the per-host cap) and `tests/http2.rs` cover each rule on its own.

## Not yet

- **https→http after confirmation:** such a redirect is refused; #67 adds the confirmation. `redirect.rs`'s unit tests cover the rule; an end-to-end test on the TLS fixture (#69) comes with #67.
- **Credentials beyond memory:** the keychain and zeroisation are #70's ([SECURITY.md](SECURITY.md#secrets-70)); the engine takes the login as plain strings for the transfer's life, copied out of its `Secret`s in `request_options`. The full secret-leak scan of every file is #71.
- **Requiring encryption from the add form:** `TransferSpec::require_encryption` and the cleartext warning are ready; the add form that sets one and shows the other is R8's. Until then, the app's FTP downloads are plain unless the link is `ftps://`.
- **FTP over several connections:** one connection until #77 measures whether more help.
- **Connection values:** the per-host cap and the smallest piece come from the performance profile (#57), and the budget from the user's engine settings ([SETTINGS.md](SETTINGS.md#engine), #30). The ramp's values are engine defaults, to tune by measurement (#77).
- **Learning per host:** what a host allowed isn't remembered between downloads (a non-goal of #47).
- **Throttling across downloads:** a `429` or `503` halves the connections of the download that got it, not of the other downloads to that host.
- **Retry settings:** the user's engine settings over `RetryPolicy::DEFAULT` ([SETTINGS.md](SETTINGS.md#engine), #30), read when a download starts or retries.
- **Integrity without the app:** a whole-file hash that a server gives (`Repr-Digest`) is deferred (report section 5.3).
- **Metalink in the app:** off by default, as in V1; the Settings screen's Advanced section turns it on (`advanced.enableMetalink`, #30).
- **Mirror health isn't remembered** between downloads, or between the engine's restarts of one download, like the per-host learning #47 left out.
- **Connections to mirrors** count against the first URL's host in the engine's budget (#47), so a download with mirrors never opens more connections than one without; they're only spread. Per-mirror budgets, and more connections across several hosts as aria2's `split` allowed, are #77's to measure.
- **Mirror values:** the cooldown, the failure limit and the scoring are starting values; #77 measures them.
- **Folders a Metalink download creates** stay when it's cancelled or fails, even when empty. Only part files are removed.
- **A fuzz target for the Metalink parser** comes with #73, which brings the fuzzing tool.
