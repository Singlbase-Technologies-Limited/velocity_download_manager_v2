# Field test

The installed app, used the way a person uses it, with real downloads from real servers and real settings (#262). It finds what the automated tests can't: servers that cut connections, long downloads, pause and resume, crashes. Every problem it finds becomes an issue in epic #1, Phase I. It runs by hand on Windows, before each release candidate, and never in CI. The test levels are in [docs/TESTING.md](../../docs/TESTING.md#the-field-test-262).

## Before a session

1. **Build and install the app from `main`:** `pnpm native:build`, then `pnpm package:windows` ([docs/PACKAGING.md](../../docs/PACKAGING.md#the-windows-installer)). Install it per-user with `/S`, or by hand. It replaces the V2 already installed; its settings and downloads are kept.
2. **Quit the app** from the tray if it's running. It runs one instance, so a running app without the debugging port can't be driven.
3. **Agree the data budget with the PO:** at most 20 GB a session unless the PO says otherwise. Delete the big files after each scenario, with the app's own *Delete files of selected…*.

## Running a session

```sh
pnpm field start                       # the installed app, with WebView2's debugging port on 127.0.0.1
pnpm field onboard [folder]            # the first-run setup, after a clean install
pnpm field add <link> [--connections N] [--folder DIR]
pnpm field watch --every 5 --for 600   # one line per download every 5 s, and samples.jsonl
pnpm field pause|resume|cancel|retry|remove <name>
pnpm field click <role> "<name>"       # any other control, by its accessible role and name
pnpm field status | log --last 50 | export | shot <file>
pnpm field quit                        # as the tray's Quit
pnpm field kill [app|engine|torrent]   # end a process at once, as a crash does
```

- **Actions are real clicks** in the real window ([`session.mjs`](session.mjs), Playwright over the Chrome DevTools Protocol).
- **Readings use the app's read-only commands** (`get_snapshot`, `get_diagnostics`), plus the part file's length on disk and the app's processes and TCP connections (PowerShell's `Get-Process` and `Get-NetTCPConnection`).
- **`watch` flags what looks wrong** ([`field-lib.mjs`](field-lib.mjs)) and takes a screenshot each time:
  - progress that goes backwards without a cancel or retry;
  - a size that changes;
  - a completed download that changes again;
  - 30 s without a byte while it still shows downloading.
- **Everything a session writes** goes to `%TEMP%\velocity-field\<date>\`, or `--out`: samples, findings, screenshots, the diagnostics export and the session's notes. Nothing goes into the repository.

## Keeping it safe

- **The debugging port** listens on `127.0.0.1` only, and only while an app started by `pnpm field start` runs. While it's open, any program on this PC can control the app, so sessions run only with the PO's knowledge. Quit the app when the session ends. The installed app and its configuration are never changed.
- **Links stay private.** The PO's own links, such as tokenised file hosts, are pasted in the session they're for. Nothing the tool writes holds a link, only its scheme and host (`hideLinks`). Screenshots can show the details panel's link, so they stay in the session folder: crop or skip them before attaching one to an issue.
- **The PO's data:** the tool never edits the app's database. To read a checkpoint, copy `velocity.sqlite3` with its `-wal` file and open the copy read-only. Only files the session downloaded are deleted, through the app.

## After a session

- **One issue per bug,** in epic #1, Phase I, with its milestone, its steps and its evidence. New evidence for a bug that's already open goes on that issue as a comment.
- **A report on #262:** the build, the scenarios run (PASS, FAIL or NOT RUN, from [CHECKLIST.md](CHECKLIST.md)), what was found, and the issues filed or updated.
