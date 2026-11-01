# Field test scenarios

Each scenario is something a user does, and together they cover every feature in the [user guide](../../docs/user/README.md) (PO decision on #262: the whole app). A session runs as many as its time and data budget allow: 20 GB, or up to 100 GB for the load and long-run groups. Its report on #262 marks each one PASS, FAIL or NOT RUN ([README.md](README.md)). A scenario that failed is repeated in the next session after its fix.

Unless a scenario says otherwise:

- **What to watch:** `pnpm field watch`, the app's log, and the part file on disk.
- **What a PASS needs:** the download completes, and its file matches the publisher's checksum where there is one. What the app shows agrees with what happened, and each error says what to do.

## Install and setup

| Id | Scenario | What to check |
|---|---|---|
| I1 | A clean install, then the first-run setup | The engine shows as running; the chosen folder is used |
| I2 | An upgrade: install v0.1.0, add downloads (finished, paused and failed), then install the new build over it | Every download, setting and category is kept; a paused one resumes ([installing.md](../../docs/user/installing.md#upgrading)) |
| I3 | V1 installed and running beside V2 | Both run; neither takes the other's browser link or files ([v1-and-v2.md](../../docs/user/v1-and-v2.md)) |
| I4 | Uninstall | The programs and the browser registrations go; downloaded files stay |

## Direct downloads

| Id | Scenario | What to check |
|---|---|---|
| D1 | A large file from a fast public server (the Ubuntu server image, 2.9 GB) | Time, connections used, SHA-256 against Ubuntu's `SHA256SUMS` |
| D2 | Public speed-test files from several hosts (proof.ovh.net, Hetzner, others) | Retries and their reasons; compare with `curl.exe` on the same file |
| D3 | Pause and resume midway, twice | The first progress after each resume isn't below what was paused with; the checkpoint's durable bytes; the total time; the checksum |
| D4 | Kill the app mid-download (`pnpm field kill app`), then start it | No engine left running; the download comes back by itself; the checksum |
| D5 | Kill the transfer engine mid-download (`pnpm field kill engine`) | The app shows it, brings the engine back, and the download resumes |
| D6 | Change the connections and the speed limit of a running download | Both apply at once, without a restart; the speed holds to the limit |
| D7 | Several downloads at once, more than the queue allows | The queue's limit; the rest wait as Queued and start in order; moving one up or down |
| D8 | Plain `http://`, a redirect, and a server that ignores ranges | Each completes; the one without ranges uses one connection |
| D9 | A link that expires, and the PO's tokenised link (pasted by the PO) | A clear failure once it expires; a correct resume while it's valid |
| D10 | Remove a running download; delete a finished one's files | The confirmations; only that download's files are deleted |
| D11 | Add a link whose file name is already taken in the folder | The new name, or the question the app asks |
| D12 | Several links pasted at once, one per line | Each is added; a bad one is reported on its own |
| D13 | A checksum given when adding | A match completes; a wrong checksum fails with that reason |
| D14 | Extra headers, a referrer and a login when adding (a test server that needs them) | The download works; none of them appears in the log, the export or the database |

## Other protocols

| Id | Scenario | What to check |
|---|---|---|
| P1 | SFTP from this PC's own sshd (WSL), with a key | An unknown host key is asked about first; the file arrives |
| P2 | SFTP with a password login | The login is asked for and kept only as the settings say |
| P3 | A public FTP server's test file, and FTPS where available | It completes |
| P4 | A Metalink file, with Metalink turned on in Settings | The files it lists are added and verified |

## Torrents

| Id | Scenario | What to check |
|---|---|---|
| T1 | Ubuntu's official torrent | Peers, speed, the helper's memory |
| T2 | Pause and resume a torrent | Progress kept |
| T3 | A torrent whose file already exists in the folder | The app refuses it and says why |
| T4 | A magnet link | The metadata arrives, then the download starts |
| T5 | Choose files before downloading, and change the choice later | Only the chosen files are downloaded |
| T6 | Seeding after the download: each choice, and an upload limit | It seeds, and stops, as chosen |
| T7 | A `.torrent` file dropped on the window | It's added |

## The browser extension

| Id | Scenario | What to check |
|---|---|---|
| B1 | Register Chrome or Edge in Settings, then load V2's extension | The extension shows as connected ([browser.md](../../docs/user/browser.md)) |
| B2 | Click a download link in the browser | V2 takes it, with its name; the browser doesn't download it as well |
| B3 | A link V2 can't take, and the app not running | The browser keeps the download, and the reason is shown |

## Settings

| Id | Scenario | What to check |
|---|---|---|
| C1 | Download folder, categories by file type, and a folder per category | New downloads go where the settings say |
| C2 | Your own category, edited and deleted | Downloads follow it, and deleting it moves nothing on disk |
| C3 | Queue and performance: the number at once, the profile, global speed limits | They apply to running downloads |
| C4 | The schedule | Downloads start and stop with the window |
| C5 | A proxy (HTTP and SOCKS5), with a login | Downloads go through it; the password is kept only as the settings say |
| C6 | Notifications | One for a finished and one for a failed download |
| C7 | Appearance: light, dark, system theme, and layout | Each applies, and is kept after a restart |
| C8 | History and storage: the retention, clearing the history | Old entries go as set |
| C9 | Advanced: retries and timeouts, the user agent, full preallocation | Each applies to new downloads |
| C10 | Start with Windows, and close to the tray | As set, after a Windows sign-in |

## Views

| Id | Scenario | What to check |
|---|---|---|
| V1 | Every list view (all, downloading, queued, completed, seeding, failed) and the category views | The counts match the list |
| V2 | Search by name, link and folder; sort; choose and resize columns | Results are right, and the layout is kept after a restart |
| V3 | The details panel for each status | Its numbers agree with each other (#245) |
| V4 | The history: search, open, remove | As the guide says |
| V5 | Keyboard only: add, select, pause, resume, delete | Everything is reachable, and focus is visible |
| V6 | A long error, a long file name, the smallest window | Nothing runs past its box (#248) |

## Robustness

| Id | Scenario | What to check |
|---|---|---|
| R1 | Turn the network off for a minute mid-download, then on | Stalled, then Retrying with a reason, then it continues |
| R2 | Sleep and wake the PC mid-download (with the PO) | It continues, or retries and continues |
| R3 | Restart Windows mid-download (with the PO) | After sign-in, it resumes from its checkpoint |
| R4 | A disk that fills up mid-download (a small test drive or folder quota) | It fails saying the disk is full, and keeps what was downloaded |
| R5 | A folder without write permission | Refused with a clear reason, before anything is downloaded |
| R6 | Kill the torrent helper mid-download | The app shows it and brings it back |

## Load and long runs (up to 100 GB)

| Id | Scenario | What to check |
|---|---|---|
| L1 | 200 small files queued at once | All complete; the app stays responsive; the queue keeps its limit |
| L2 | 20 large downloads at once | Speed, memory and CPU; none fails without a reason |
| L3 | An overnight run: large downloads in a loop for 8 hours or more | Memory and handles stay flat; the log stays readable |

## Diagnostics and privacy

| Id | Scenario | What to check |
|---|---|---|
| G1 | The app's log after each scenario (`pnpm field log`) | Every step and every retry is there with its reason, and no link appears whole |
| G2 | The diagnostics export (`pnpm field export`) | It holds the app's and the engines' logs |
| G3 | After the session, a search for every login, token and tokenised link used, in the log, the export and the database | None is found |
