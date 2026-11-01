# Field test scenarios

Each scenario is something a user does. A session runs as many as its data budget allows, and its report on #262 marks each one PASS, FAIL or NOT RUN ([README.md](README.md)). Unless a scenario says otherwise:

- **What to watch:** `pnpm field watch`, the app's log and the part file on disk.
- **What a PASS needs:** the download completes, and its file matches the publisher's checksum where there is one.

## Setup

| Id | Scenario | What to check |
|---|---|---|
| S1 | A clean install, then the first-run setup | The engine shows as running; the chosen folder is used |

## Direct downloads

| Id | Scenario | What to check |
|---|---|---|
| D1 | A large file from a fast public server (the Ubuntu server image, 2.9 GB) | Time, connections used, SHA-256 against Ubuntu's `SHA256SUMS` |
| D2 | Public speed-test files from several hosts (proof.ovh.net, Hetzner, others) | Retries and their reasons; compare with `curl.exe` on the same file |
| D3 | Pause and resume midway, twice | The first progress after each resume isn't below what was paused with; the checkpoint's durable bytes; the total time; the checksum |
| D4 | Kill the app mid-download (`pnpm field kill app`), then start it | No engine left running; the download comes back by itself; progress; the checksum |
| D5 | Kill the transfer engine mid-download (`pnpm field kill engine`) | The app shows it, brings the engine back, and the download resumes |
| D6 | Change the connection count of a running download | The new count applies without a restart |
| D7 | Several downloads at once (five or more) | The queue's limit; each one's speed and connections |
| D8 | Turn the network off for a minute mid-download, then on | Stalled, then Retrying with a reason, then it continues |
| D9 | A link that expires, or the PO's tokenised link (pasted by the PO) | A clear failure, or a correct resume while the link is valid |
| D10 | Remove a running download, and delete a finished one's files | The confirmations; only that download's files are deleted |

## Torrents

| Id | Scenario | What to check |
|---|---|---|
| T1 | Ubuntu's official torrent | Peers, speed, the helper's memory |
| T2 | Pause and resume a torrent | Progress kept |
| T3 | A torrent whose file already exists in the folder | The app refuses it and says why |

## SFTP and FTP

| Id | Scenario | What to check |
|---|---|---|
| P1 | SFTP from this PC's own sshd (Windows or WSL), with a key | An unknown host key is asked about first; the file arrives |
| P2 | A public FTP server's test file | It completes |

## Diagnostics

| Id | Scenario | What to check |
|---|---|---|
| G1 | The app's log after each scenario (`pnpm field log`) | Every step and every retry is there with its reason, and no link appears whole |
| G2 | The diagnostics export (`pnpm field export`) | It holds the app's and the engines' logs |
