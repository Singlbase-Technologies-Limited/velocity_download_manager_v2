# Privacy

What Velocity Download Manager V2 keeps, and what it sends. AGENTS.md's rule is that V2 makes no network calls beyond the downloads themselves without a PO decision.

## What V2 sends

- **Only the downloads.** V2 connects to the servers, mirrors, trackers and peers of the downloads you add, and to nothing else.
- **No telemetry or analytics.** V2 has none. Adding any needs a PO decision first.
- **No crash reports.** A crash is recorded on this computer only ([DIAGNOSTICS.md](DIAGNOSTICS.md#crash-logs)).
- **The browser extension** (#64, #65) talks only to the app on this computer, through the native host and a local connection that refuses remote clients. It sends the link, and the referrer, file name, size and type when the browser knows them; never cookies, logins or page content.

## What V2 keeps on this computer

All of it is in V2's own data folder ([COEXISTENCE.md](COEXISTENCE.md)):

- **The downloads and their history:** links with any login removed, folders, file names, statuses and times ([DATA_AND_MIGRATIONS.md](DATA_AND_MIGRATIONS.md)). The history keeps them for the retention you set.
- **Settings and categories.**
- **Checkpoints and torrent resume data,** so downloads resume after a restart. Torrent resume data is kept without trackers.
- **Trusted SFTP host keys,** in V2's own `known_hosts` file.
- **Crash logs:** at most 5, redacted.

**Never kept:** passwords, cookies, custom headers, SSH keys and passphrases. They stay in memory for the session only, and a download that needs one asks again after a restart.

## Diagnostics you share

**In memory only:** the app's log and the engines' logs, with secrets removed ([DIAGNOSTICS.md](DIAGNOSTICS.md#redaction)). They are lost when V2 closes.

**Exporting:** "Export diagnostics…" writes a report to a file you choose. It names no download, link or folder: it counts downloads by status only. Logins, cookies, keys, tokens, magnet links and tracker passkeys are removed from every line.

**Sharing:** you decide whether to share the file, and with whom.
