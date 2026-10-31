# Troubleshooting

What to do when a download doesn't go as expected. Every message a download can show, with its cause and what to do, is in the [complete list of download messages](../TROUBLESHOOTING.md#downloads).

## A download waits

| What you see | What it means | What to do |
|---|---|---|
| **Retrying in …** or **Waiting to retry** | The download failed in a way worth retrying | Nothing: Velocity tries again by itself. If it ends as Failed, the message says why |
| **No data received** | The server stopped sending | Nothing at first: after 30 seconds Velocity resets the connection and tries again. If it keeps happening, check your network connection |
| **Queued** | The queue already runs as many downloads as Settings allows (3 by default), or the schedule is outside its hours | Wait, or raise **Simultaneous downloads** in Settings, Queue and performance |
| **Needs its login** | Velocity restarted, and didn't keep the download's login, headers or key | **Resume** it and enter them again. Tick **Remember this login** next time |
| A notice that the transfer engine isn't running | The engine stopped, and the downloads wait for it | Usually nothing: Velocity starts it again. If it crashed more than 5 times in 5 minutes, use **Restart transfer engine** in Settings, About and diagnostics |
| **Waiting for peers** (a torrent) | No one is sharing the torrent right now | Wait. A private torrent added before a restart may need to be added again ([Torrents](torrents.md#not-yet-or-different-from-v1)) |

## A download stopped and asks something

| What you see | What to do |
|---|---|
| **Trust this server?** (SFTP) | Check the fingerprint with the server's owner first ([Downloads](downloads.md#sftp-servers)) |
| The server redirected from `https://` to `http://` | The rest would come over an unencrypted connection. **Continue over http** in the details panel goes on for this download only. Choose it only if you trust the network |
| A link from the browser to this computer or the local network | Velocity holds links from the browser to addresses such as `127.0.0.1` or `192.168.x.x` before sending anything. **Download anyway** in the details panel goes on |
| A name already taken | **Keep both (rename)** or **Skip**: Velocity never replaces a file |

## A download failed

The details panel shows the message, the server's HTTP status when there is one, and whether retrying can help. The most common:

| Message | What to do |
|---|---|
| "The file was not found on the server (404). The link may have expired." | Get a new link from the site |
| "The download link expired (403). Get a fresh link from the browser." | Copy the link again from the site, and add it again |
| "Access was denied by the server (403)." or "The server requires authentication (401)." | The link needs a login: add it again with your user name and password, or get a new link |
| "The server's certificate isn't trusted, so the connection was refused." | Check your computer's date and time, and whether a proxy or security program intercepts connections. Otherwise the site must fix its certificate. Velocity never skips this check |
| "The server's host key changed …" (SFTP) | Ask the server's owner whether the key changed, and why. Velocity never trusts a changed key: after checking, remove the old entry from the `known_hosts` file in Velocity's data folder by hand |
| "The downloaded file doesn't match its SHA-256 checksum …" | Check that the checksum is the one the site gives for this exact file. The unfinished file is kept as `<name>.vdmpart` |
| "There is not enough disk space." | Free some space, then **Retry**: the download goes on from where it stopped |
| "Velocity isn't allowed to write to that location." | The folder stopped accepting writes (its permissions changed, or the drive became read-only). Give your user write access again, then **Retry** |
| "The proxy refused the connection. Check its user name and password in Settings, Proxy." | Check the proxy's login, then retry. Ask the proxy's administrator whether it allows that site |
| "The server can't encrypt the connection, and this download requires encryption." | The FTP server doesn't offer FTPS. Ask for an `ftps://` or `https://` link |
| "The destination file already exists; it is never replaced" | A file appeared at the download's name while it downloaded. Velocity kept it. Move or rename that file, or add the download again with another name |

After a disk-full or write error, if Velocity hadn't saved any progress yet, the retry starts the file over.

## The browser doesn't send downloads

See [Browser integration](browser.md#when-v2-cant-take-a-link). The extension's popup says why: V2 isn't running, or that browser isn't registered in V2's Settings, Browser.

## The diagnostics report

**Settings, About and diagnostics** shows the versions of Velocity and its engines, what the download engine supports, and the recent log. **Export diagnostics…** saves a report to a file you choose, to attach to a bug report.

- **Secrets are hidden:** passwords, cookies, tokens, tracker passkeys and keys are removed from the report and from the log before anything is kept.
- **Crash logs** are kept on this computer only, in their own folder, newest first. Velocity never sends anything anywhere: no telemetry, no automatic reports.
- **Restart transfer engine** and **Restart torrent helper** are there too, and ask first.

## If Velocity closes suddenly

After a crash, a forced quit or a power cut, the downloads come back when Velocity starts again and continue from their last saved progress.
