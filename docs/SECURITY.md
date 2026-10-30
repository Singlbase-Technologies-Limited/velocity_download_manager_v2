# Security

How V2 protects downloads and the people who use it. The rules every contributor follows are in [AGENTS.md](../AGENTS.md#security-and-data) and `.claude/rules/security.md`. This document records what V2 does and what was measured. It covers TLS, links and redirects, the browser bridge, FTP, SFTP host keys, secrets, files and folders, and dependency advisories.

## TLS verification (#69)

The direct-download engine verifies every server certificate, with the platform's TLS stack: **Schannel** on Windows and **OpenSSL 3.5** on Linux ([BUILDING_NATIVE.md](BUILDING_NATIVE.md)). Verification is never turned off, in production or in a test, and there is no setting for it.

- **Trust:** on Windows, the Windows certificate store. On Linux, the CA bundle compiled into libcurl, `/etc/ssl/certs/ca-certificates.crt`.
- **Revocation:** Schannel checks it. libcurl with OpenSSL checks none, because V2 gives it no CRL file (`CURLOPT_CRLFILE`). See [revocation](#revocation).
- **A refused certificate** fails the download at once, as permanent, with "The server's certificate isn't trusted, so the connection was refused." and libcurl's own reason. Nothing is written: no file, no part file ([TROUBLESHOOTING.md](TROUBLESHOOTING.md#downloads)).

### What was measured

The certificate tests run real libcurl against the local TLS fixture ([TESTING.md](TESTING.md#fixture-servers-v2-needs)). Each case asserts the reason the backend gives, so a certificate refused for the wrong reason fails the test.

| Server certificate | Windows (Schannel) | Linux (OpenSSL 3.5) |
|---|---|---|
| Valid, for server authentication | Accepted | Accepted |
| No extended key usage (any purpose, RFC 5280 §4.2.1.12) | Accepted | Accepted |
| Client authentication only | Refused: `error 0x00000010` (`CERT_TRUST_IS_NOT_VALID_FOR_USAGE`) | Refused: "unsuitable certificate purpose (26)" |
| Code signing and e-mail protection only | Refused: `error 0x00000010` | Refused: "unsuitable certificate purpose (26)" |
| Untrusted issuer | Refused: "the certificate chain is incomplete" | Refused: "unable to get local issuer certificate (20)" |
| Another host (`wrong-host.invalid`) | Refused: "failed to match connection hostname (localhost) against server certificate names" | Refused: "no alternative certificate subject name matches target hostname 'localhost'" |
| Expired (2000-01-01 to 2000-01-02) | Refused: "… is not time valid" | Refused: "certificate has expired (10)" |
| Revoked, on the CRL the certificate names | Refused: "… has been revoked" | **Accepted** (no CRL check) |
| Valid, but the test CA not given | Refused: `SEC_E_UNTRUSTED_ROOT` | Refused: "unable to get local issuer certificate (20)" |

- **Purpose checks** (the CVE-2026-8367 class): both backends refuse a certificate from a trusted CA that isn't for TLS servers. The research report's open question 1 asked this of Schannel; it is answered here.
- **TLS 1.3:** both backends negotiate TLS 1.3 with the fixture, recorded on the server side.
- **HTTP/2** is negotiated through ALPN over TLS on both, and a segmented download over it completes.
- **Measured on** Windows 11 (`check:pr` locally, and PR checks' Windows job) and Linux (the WSL clean clone, and CI). **macOS: NOT RUN** (#83).

### Revocation

- **Windows:** Schannel checks every certificate's revocation through its CRL distribution point, and refuses one whose status it can't learn: "the revocation status is unknown" (found on #47). A server whose certificate names no reachable CRL or OCSP responder is therefore refused on Windows. Windows caches the lists it fetched.
- **Linux:** libcurl with OpenSSL checks revocation only with a CRL file, and V2 sets none, so a revoked certificate is accepted there.
- **This is recorded, not decided.** Whether V2 should check revocation on Linux (a CRL, OCSP, or stapling), or relax anything on Windows, is a decision for the PO; nothing here changes either.

### The Windows certificate store

Users on Windows reach servers through the Windows store, not a CA file. A test adds the test CA to the machine's Root store, downloads from a valid certificate, is refused a revoked, expired, wrong-host, untrusted-issuer and client-auth-only one, and removes the CA.

Measured on PR checks' Windows runner (run `40072038728`):

| Certificate, through the store | libcurl code | Schannel's reason |
|---|---|---|
| Valid | — | Accepted |
| Revoked | 35 | `CRYPT_E_REVOKED (0x80092010)` |
| Expired | 35 | `SEC_E_CERT_EXPIRED (0x80090328)` |
| Client authentication only | 35 | `SEC_E_CERT_WRONG_USAGE (0x80090349)` |
| Another host | 60 | `SEC_E_WRONG_PRINCIPAL (0x80090322)` |
| Untrusted issuer | 60 | `SEC_E_UNTRUSTED_ROOT (0x80090325)` |

- **A different error code on this path:** through the store, Schannel reports a refused certificate as `CURLE_SSL_CONNECT_ERROR` (35), with the reason only in its detail, for example `CRYPT_E_REVOKED (0x80092010)`. With a CA file it reports `CURLE_PEER_FAILED_VERIFICATION` (60). The engine treats both as a refused certificate: permanent, never retried. The store test found the first one classified as a transient handshake failure on CI, and the fix came in the same PR, with a regression test. It needs an administrator, so it is ignored locally and run by PR checks' Windows job ([CI.md](CI.md)). Locally, as an administrator: `cargo test -p velocity-curl --test windows_store -- --ignored`. If it can't remove the CA, it says so ("REMOVE BY HAND").

### How the tests trust a test CA

- **Fresh per test:** each TLS front makes its own CA, with a unique name, and serves its own CRL on a fresh port, so a list cached by Windows never answers for another run.
- **Test-only trust:** the tests pass the CA file with velocity-curl's `test-ca` feature (`CURLOPT_CAINFO`, PO decision on #69). It replaces the platform's store for that test; verification and revocation checks stay on. An optimised build refuses to compile with the feature, so a release can't contain it, like `test-h2c`.
- **Nothing installed** on the machine, except in the Windows store test above.
- **The FTPS server** terminates TLS itself, with the same kind of test CA, leaf and revocation list (`TestPki`), since `AUTH TLS` starts TLS part way through a connection.

## Links, redirects and local addresses (#67)

Links come from users and from web pages, so each is checked before it reaches the engine, and every request the engine makes is held to an allowlist.

- **Checking a link** (`request::normalize_link`, on every link the Add path takes): a space or a control character is refused with `INVALID_URL`, as V1 refused it. A link longer than 8 KiB is refused, and so is a login in the link, which has its own fields (#40). An international domain name is turned into its ASCII form, so `https://bücher.example/` is stored, shown and requested as `https://xn--bcher-kva.example/` (PO decision on #67, with the `url` crate's IDNA). A look-alike name therefore shows as what it is. Other characters outside ASCII are percent-encoded.
- **Protocols:** every libcurl request allows only `http`, `https`, `ftp` and `ftps` (`CURLOPT_PROTOCOLS_STR`), and a redirect only `http` and `https` (`CURLOPT_REDIR_PROTOCOLS_STR`). V2 follows redirects itself and checks each one first (`redirect.rs`): a redirect to `file://`, `scp://`, `sftp://` or any other protocol is refused before anything is connected (tested against the local server; CVE-2026-86410 was a redirect to SFTP in the curl tool). At most 10 redirects are followed. SFTP links go to V2's own SFTP layer, never through libcurl.
- **From `https` to `http`:** a redirect that would leave TLS stops the download with "The server redirected to an insecure http:// address on <host>…", and the details panel offers **Continue over http** (PO decision on #67). Choosing it allows the downgrade for that download only, is stored with it, and retries it. The login and custom headers still don't follow, since `http` is another origin. Tested against the local TLS fixture, with and without the confirmation.
- **Local addresses** ([ADR-0006](decisions/0006-browser-links-to-local-networks.md)): a link the browser extension sent (`AddRequest.fromBrowser`) that connects to this computer or a private network stops before its request is sent, and the details panel shows the address it really leads to, with **Download anyway**. Choosing it allows local addresses for that download only, is stored with it, and retries it. Local means loopback, private (RFC 1918), shared (RFC 6598), link-local, unspecified and IPv6 unique-local addresses, also when mapped into IPv6. The check is on the address libcurl actually connected to (`CURLOPT_PREREQFUNCTION`), after the name was resolved, so a name that resolves to a local address, as in DNS rebinding, is caught too (tested with `localhost`). Typed and pasted links are never checked.
- **No ambient configuration:** proxy variables such as `http_proxy` are ignored, because every request sets an empty proxy (ADR-0008; the proxy feature is #192), and `.netrc` support is compiled out of V2's libcurl (`scripts/native/build.mjs`). Both are tested in their own test binaries, which set the variables and a `.netrc` with a login and check the request went direct and without it.
- **Logs:** a refused redirect's message names no URL, which may hold a token; the libcurl layer never logs one.

## The browser bridge (#65)

Messages from the browser are untrusted, and reach the app only through V2's own native host ([BROWSER_EXTENSION.md](BROWSER_EXTENSION.md#the-native-host)):

- **Only V2's extensions:** the host and the app accept only V2's Chrome extension origin and Firefox add-on ID. V1's are refused (tested).
- **Checked twice:** the host checks each message against the protocol's allowlist (three commands; no paths, headers, cookies or engine options; at most 64 KiB), and the app checks it again.
- **Authenticated:** the app writes a per-launch token (32 bytes from the OS's secure random source) where only this user can read it, and refuses an envelope without it (compared in constant time). The Windows pipe is created first and refuses remote clients; the Unix socket is owner-only.
- **Bounded:** at most 8 connections at once, 15 seconds each, a line of at most 72 KiB, and 30 messages in 10 seconds.
- **The user decides:** by default a link only opens the Add dialog, and torrents always do. A link added at once is marked as from the browser, so a local address waits for the user ([above](#links-redirects-and-local-addresses-67)).
- **Registration** is per user, and never touches V1's: every file and registry key is named after V2's host, and unregistering removes only a plain file that names V2's host.

## FTP and FTPS (#51)

How FTP downloads work is in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#ftp-and-ftps-51). What protects them:

- **Plain FTP is cleartext.** The login and the file travel unencrypted, so anyone on the network path can read them. The engine reports what it can encrypt per protocol (`encryption` in the capabilities), and the add form (R8) warns before a plain `ftp://` download starts (PO decision on #51).
- **Requiring encryption never falls back.** A download that requires it sends `AUTH TLS` before the login and fails at once if the server refuses, with nothing sent in clear after the greeting: no login, no file name. `ftps://` is TLS from the first byte. Both protect the data connections too (`PROT P`). Tested on Windows and Linux: a plain server, asked for encryption, never receives the login.
- **Certificates** are verified as in [TLS verification](#tls-verification-69), with the same backends, store and revocation behaviour. A refused certificate fails before the login.
- **No bounce to another host:** the data connection goes to the control connection's address; the address in a `PASV` reply is ignored. Active mode (`PORT`), where the server connects back, is never used.
- **The login stays out of storage.** It reaches the engine as `credentials`, and a login in the link is split off before the download is stored. The database test scans every file SQLite wrote for an FTP login, in the link and as credentials. The libcurl layer reads only the verb of each command it sends, so the `PASS` line isn't kept or logged.
- **An `http://` link can't require encryption:** such a spec is refused, rather than sent in clear.
- **Not verified:** servers that require the data connection to resume the control connection's TLS session ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#ftps-the-evaluation-51-asked-for)).

## Mirrors (#55)

A download's mirrors, such as a Metalink file's, are other servers than the one the user chose. How they're used is in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#mirrors-55). What protects V2:

- **No bytes are mixed unless proven the same.** A mirror serves a range only after its first answer shows the same size and encoding, and, without trusted piece hashes, the same strong validator as the download's first answer. With piece hashes, every piece is checked, and a mirror that sends other bytes is disabled after its second bad piece. Tested: a mirror with other bytes of the same size never ends up in the file.
- **No credentials to other servers.** The login and custom headers go only to the first URL's origin; a mirror on another origin gets neither (tested). A Metalink download carries none anyway (#54).
- **Every mirror is checked** like the first URL: the scheme allowlist applies to each, and one the engine may not fetch, such as `file://`, refuses the download (tested).
- **A bad mirror can't stop a download** as long as another source is left: it is cooled down or disabled, and its ranges go elsewhere.

## Metalink (#54)

A Metalink document is untrusted input, from a file the user chose or from any link. How it's read and used is in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#metalink-54). What protects V2:

- **No entity can expand.** A document with a `<!DOCTYPE>` is refused, so it can't declare an entity: no "billion laughs", and no external entity that reads a local file or a URL. Only XML's five predefined entities and character references are read. The parser is quick-xml (PO decision on #54), which never expands custom entities itself.
- **Bounded:** at most 16 MiB, 10,000 files, 256 sources per file, 32 levels of nesting and 8 KiB of text per value, read as a stream. A piece list fits the engine's limit (2 MiB of digests).
- **Safe names:** every part of a file's name must be one safe name (`naming::check_file_name`): no `..`, no absolute path, no drive letter, no `\` or `:`, no reserved Windows name, no control character, no part file's name, at most 32 subfolders deep. Two files at one path, whatever their case, or a file where another file's folder is, refuse the document. One bad name refuses the whole document, never only its file.
- **Folders inside the chosen folder:** created one level at a time; a file or a link in the way is refused, never followed (tested with a symbolic link on Linux and a junction on Windows).
- **No credentials from a document:** a source with a login in it is dropped, and a Metalink download sends no login, custom header or referrer. SFTP sources are dropped, since they would need a login and a host key the user confirms.
- **Off by default,** as in V1, so a link that turns out to be a Metalink file is just a file until the user turns Metalink on.

## Adding downloads (#60)

Everything the Add dialog sends is checked again natively (`Manager::add_request`); the dialog's own checks are for usability. What protects V2:

- **A login never goes in a link.** A link with one is refused (`INVALID_URL`); the login has its own fields and stays in memory, as do custom headers and a key's passphrase.
- **`.torrent` files are read only as plain files:** an absolute path, never a link, at most 16 MiB, parsed by V2's own bounded parser before the torrent helper sees it. Files dropped on the window reach Rust, which passes the page only plain `.torrent` files, at most 500; the manager checks each again when it is added.
- **A chosen folder is created only when the user agrees,** inside its nearest existing parent, one level at a time, never through a link.
- **Fetched `.torrent` links** go through the transfer engine with its usual TLS checks, into the app's own `torrent-links` folder, at most 16 MiB and 60 seconds. What arrives must parse as a torrent. It is kept without its trackers, whose addresses can carry a private tracker's passkey; they stay in memory, as every torrent's do (PO decision on #58). When the app starts, it deletes any plain file in that folder that no download comes from, and touches nothing else there.
- **SFTP host keys** are confirmed by the user in the window, after the download stops before logging in (below). A changed key is only explained, never offered for trust.
- **A login given again** (`set_download_login`, #59), for a download that came back after a restart without it, is checked as the Add path checks it (`TransferSpec::check_request`): its headers, its login, and that an SSH key is for an SFTP link with a user. An invalid one is refused whole, and the download keeps waiting for it. It stays in memory, as when the download was added.

## SFTP host keys (#53)

SSH has no certificate authority to vouch for a server, so a host key is trusted only once the user has seen and confirmed its fingerprint, and it is pinned from then on (research report section 8). How it works is in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#host-keys); the backend is russh ([ADR-0013](decisions/0013-sftp-russh.md)).

- **Nothing before the key is checked:** the host key is checked against V2's own known_hosts file before the login, so no password, key signature or file name goes to an unverified server.
- **Unknown keys wait for the user.** The download fails with the key's SHA-256 fingerprint, and only the key the engine actually saw with that fingerprint can be trusted.
- **Changed keys are always refused,** and are never offered for trust. A pinned key is never replaced by trusting another.
- **V2's own file,** `<data folder>/known_hosts`, unhashed and readable. `~/.ssh/known_hosts` is neither read nor written.
- **Host certificates are refused:** V2 trusts no SSH certificate authority.
- **Logins:** a password, or a private key file with an optional passphrase. Both travel like other credentials: in memory, and through the engine's pipe, never its arguments, environment, database, checkpoints or logs ([ENGINE_CONTRACT.md](ENGINE_CONTRACT.md)). `Debug` never prints a passphrase (tested). A login can be remembered in the keychain ([below](#secrets-70)).
- **Tested** against the installed OpenSSH server on Windows and Linux, as an unprivileged `sshd` with throwaway host keys (Ed25519, ECDSA and RSA): an unknown key refused until trusted, a changed key refused, key logins with and without a passphrase, a refused login. Password logins are tested on CI's Linux runner, against an `sshd` run as root; on Windows they are NOT RUN.
- **An accepted advisory:** `rsa` (used for RSA keys) has RUSTSEC-2023-0071 ("Marvin"), a timing side channel in private-key operations, with no fix. The PO accepted it on #53: an SFTP client signs once per login with the user's own RSA key, and host-key checks use only public keys ([DEPENDENCIES.md](DEPENDENCIES.md)).

## Secrets (#70)

Logins, custom header values (cookies, tokens) and SSH key passphrases are secrets. What V2 does with them:

- **Held as `Secret`s** (`crates/velocity-core/src/secret.rs`): overwritten in memory when dropped (`zeroize`, PO decision on #70), and never shown by `Debug` (tested). The contract's JSON doesn't change: a secret crosses the window's commands and the engine's pipe as its plain string, in memory. The engine copies the login out as plain strings for the libcurl and SFTP layers, for the transfer's life only. Those copies, and the ones libcurl and russh keep while a transfer runs, are not wiped by V2.
- **Never written** to the database, a checkpoint, a temporary file, the app's log, the diagnostics export, or a child process's arguments or environment. The leak test (`crates/velocity-transfer/tests/secret_leaks.rs`) adds downloads with an HTTP login and cookie and an FTP login through the real child engine, and SFTP key and password logins, pauses one so a checkpoint is stored, then runs the shared secret-leak scan (#71, [TESTING.md](TESTING.md#the-secret-leak-scan-71)) over every file the test's folders hold (WAL included, open and after closing), temporary files, the app's log and the diagnostics export, after logging each download's record (the core itself logs nothing while a download runs). On Linux it also reads the running engine's arguments and environment; everywhere it checks the child is started with only `serve` and the few kept variables. A private tracker's passkey gets the same scan through the torrent helper (`crates/velocity-torrent/tests/manager.rs`). The SFTP password part runs where a password server is provisioned (CI's Linux runner); elsewhere it says NOT RUN, as the SFTP engine tests do.
- **The system keychain, on consent only** ([ADR-0015](decisions/0015-keychain-with-consent.md)): "Remember this login in the system keychain", unticked by default, saves that download's login, headers and key passphrase as one entry under V2's own service name (`<identifier>.logins`), through the `keyring` crate: Windows Credential Manager, or the Secret Service on Linux. After a restart the download gets them back from there; removing it deletes the entry. A keychain that is missing or refuses is said (`KEYCHAIN_UNAVAILABLE`), and nothing is saved. A saved login is checked again, as when it was added, before it is used.
- **After a restart, without a remembered login,** a download that had one waits for it again, with a note (V1's behaviour, #58).

## Files and folders (#72)

Names come from links, servers (`Content-Disposition`), browsers, torrents and Metalink documents, so they are untrusted; and the folder a download writes to may hold other programs' files, V1's included. What V2 does:

- **One set of checks** (`crates/velocity-core/src/fs_safety.rs`), shared by the engine's part files, Metalink folders, download removal, torrent deletion and the database: whether something is a link, a plain file or a plain folder (read without following it), whether a path is inside its folder by its names alone, and whether it is reached only through plain folders.
- **What a link is:** a symbolic link, or on Windows a junction or a mount point, which redirect to another place. Other Windows reparse points, such as the files of a cloud-sync folder, are ordinary files (PO decision on #72).
- **Names stay in the folder:** a name with `..`, a separator, a drive, a reserved Windows device name (`CON`, `con.txt`, `COM1` to `COM9`, `LPT1` to `LPT9`, `CONIN$`…), a control character or a part file's suffix is refused where it is given (`naming::check_file_name`; the engine checks every spec again), and renamed predictably where V2 picks the name (`CON.txt` becomes `_CON.txt`, `../../x.sh` becomes `x.sh`). A path that climbs out with `..` is never inside its folder, whatever is on disk (a regression test: torrent deletion once compared only the start of the path).
- **Nothing through a link:** part files are created only where nothing exists (`create_new`), reopened with `O_NOFOLLOW` or `FILE_FLAG_OPEN_REPARSE_POINT` and checked again on the open handle, and folders are created one level at a time, refusing a link in the way.
- **No overwrite:** a finished file is renamed into place without ever replacing an existing file (`renameat2` with `RENAME_NOREPLACE` on Linux, `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` on Windows), so a file that V1 or anything else wrote at that name meanwhile is kept (tested). Names that look alike but are stored differently (é as one character, or e and a combining accent) are two files, as Windows and Linux treat them; names are not normalised (PO decision on #72), and finishing one never replaces the other (tested). A name differing only in case is the same file on Windows, so it is refused there and kept.
- **Long paths:** a folder deeper than Windows' old 260-character limit works: the finishing rename passes a verbatim path (`\\?\C:\…`), as the standard library's own calls do (tested with a path over 300 characters).
- **Deleting only what a download owns:** removal deletes a direct download's file and part file, and a torrent's files and the folders it made, each only if it is a plain file no larger than the download says, reached through plain folders inside the download's folder. A link at a file's name, or a download folder that has become a link, deletes nothing (tested with a junction on Windows).
- **Owner-only data:** on Linux the database, its WAL and shared-memory files (which hold the checkpoints) and its backups are `0600` (tested).
- **How the link tests run:** junctions need no rights on Windows, so folder-link cases always run. A file symbolic link needs Developer Mode or an administrator there; without either, those cases print NOT RUN (PO decision on #72). On Linux every case runs.

## Dependency advisories (#75)

V2 ships native libraries and many crates and npm packages, and advisories against them are published every week. How V2 finds them and what it does (PO decisions on #75):

- **The check:** `pnpm check:advisories` runs cargo-deny 0.20.2 (`cargo deny --locked check advisories`, configured in [`deny.toml`](../deny.toml)) against `Cargo.lock` and the RustSec database, and `pnpm audit --audit-level moderate` against `pnpm-lock.yaml`. It fails on a vulnerability, on unsound code anywhere in the graph, on an unmaintained crate V2 depends on directly, and on an npm advisory rated Medium (npm's "moderate") or higher, unless the advisory is accepted below. A yanked crate is a warning. cargo-deny is a developer tool, installed with `cargo install cargo-deny --locked --version 0.20.2` ([DEPENDENCIES.md](DEPENDENCIES.md)).
- **When it runs:** by hand, in [`advisories.yml`](../.github/workflows/advisories.yml) (`gh workflow run advisories.yml`), while CI runs by hand only ([CI.md](CI.md), #204), and weekly, Mondays 04:00 UTC, once CI runs automatically again; at least before each release candidate.
- **Native libraries** aren't in either lockfile, so they are watched by hand, at each check and before each native update ([BUILDING_NATIVE.md](BUILDING_NATIVE.md)): curl ([vulnerability table](https://curl.se/docs/vulnerabilities.html)), nghttp2, zlib and OpenSSL (the versions in [`scripts/native/deps.lock`](../scripts/native/deps.lock)), libtorrent with Boost and OpenSSL from vcpkg (#62), and SQLite, which `libsqlite3-sys` bundles (RustSec covers the crate, not every SQLite CVE). The WebView (WebView2 on Windows, WebKitGTK on Linux) is the operating system's and is updated with it. russh is a crate, so RustSec covers it.
- **Dependabot** opens security-fix PRs for Cargo and npm, and no routine version updates. Each is an ordinary PR the PO reviews.
- **Assessing one:** each advisory that affects a locked or native dependency gets an issue from the "Advisory assessment" template: where the dependency comes in, on which platforms, whether the affected code is built in and reachable, and whether it ships or is only a development tool. Then it is fixed, or accepted with its reason in `deny.toml` (or pnpm's audit configuration) and in the register below.
- **Response times,** from the day the advisory is published or found, whichever is later, to a fix merged or an accepted assessment: **Critical 2 days, High 7 days, Medium 30 days, Low with the next release.**
- **Release blocker:** an unfixed advisory rated Medium or higher that affects a feature V2 enables blocks a release or release candidate ([DEPENDENCIES.md](DEPENDENCIES.md#release-blockers-related-to-provenance)).

### The register

What the check found or accepts, checked 2026-10-30:

| Advisory | Dependency | Where it comes from | Status |
|---|---|---|---|
| RUSTSEC-2026-0418 (GHSA-8p3x-q3q3-q3v5) | libz-sys 1.1.30: its build script writes the whole build environment, secrets included, under `target/` on Linux | curl-sys, for libcurl | **Fixed** in #75: libz-sys 1.1.31 |
| RUSTSEC-2023-0071 | rsa 0.10.0-rc: a timing side channel ("Marvin") in private-key operations, no fix | russh, for SFTP RSA keys | **Accepted** (PO, #53): an SFTP client signs once per login with the user's own key; host-key checks use public keys only |
| RUSTSEC-2024-0429 (GHSA-wrw7-89jp-8q8g) | glib 0.18.5: unsound `VariantStrIter` | Tauri's GTK 3 stack, Linux only | **Accepted** (PO, #184): V2 never calls it. Waits for a Tauri release on gtk-rs 0.20 (#184) |
| RUSTSEC-2024-0370 | proc-macro-error 1.0.4: unmaintained | glib-macros 0.18, Tauri's GTK 3 stack, Linux only | **Listed** (PO, #75): a build-time macro crate; moves with Tauri |
| RUSTSEC-2026-0370, RUSTSEC-2026-0371, RUSTSEC-2026-0372 | zvariant 5.15.1, zbus_names, zvariant_derive: unmaintained | zbus 5.19, for the Secret Service (keyring) and notifications (notify-rust) on Linux | **Listed** (PO, #75): moves with keyring and tauri-plugin-notification |
| (yanked) | crypto-bigint 0.7.7: yanked by its authors | russh 0.64.1 and its elliptic-curve crates | **Listed:** a warning; moves with russh |

npm: `pnpm audit` found no known vulnerabilities on 2026-10-30.
