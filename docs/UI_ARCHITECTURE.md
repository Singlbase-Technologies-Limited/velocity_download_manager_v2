# The UI

How the window (`src/`) is built: where its state lives, how it talks to the native side, and how it is styled and tested. The commands and types it uses are in [CONTRACTS.md](CONTRACTS.md), and the settings they save are in [SETTINGS.md](SETTINGS.md). The shell came with #29, as a port of V1's ([PARITY.md](PARITY.md)).

## The parts

| Folder | What it holds |
|---|---|
| `src/app/` | The window as a whole. `App` handles starting up, `Shell` the window once it is set up, plus `Sidebar`, `StatusBar`, the theme hook and the error screen |
| `src/features/` | One folder for each screen: `onboarding/` (the first-run setup), `categories/` (the Categories screen and its editor), `add/` (the Add download dialog and the SFTP host-key question, #60), and `downloads/` (the download list, its menus and the login dialog, #59) |
| `src/components/` | Shared pieces, one per file: `Button`, `Dialog`, `ConfirmDialog` and `confirmAction`, `Field`, `Input`, `Textarea`, `Select`, `Checkbox` (the form controls share `fieldStyles.ts`), `Splitter`, `EmptyState`, `Spinner`, `Badge` (with a status tone), `ProgressBar`, `Menu` (`ContextMenu` and `DropdownMenu`), `BrandIcon` and `CategoryGlyph` |
| `src/layout/` | The panel rules (`layoutLogic.ts`, V1's), and saving the layout (`useLayout.ts`) |
| `src/lib/` | The bridge to Rust (`api.ts`, `events.ts`, `errors.ts`), the queries (`queries.ts`), and `format.ts` and `cn.ts` |
| `src/stores/` | Presentation state (`ui.ts`), and the latest snapshot of the downloads with the list's selection, search and sort (`downloads.ts`) |
| `src/downloads/` | Rules about downloads that more than one part uses: the counts for each list (`views.ts`), and searching, sorting, the actions each status allows and the status labels (`list.ts`) |
| `src/contract.ts` | The strict schemas for everything the native side sends ([CONTRACTS.md](CONTRACTS.md)) |

## Who owns which state

- **The native side owns the data:** settings, categories and downloads. The window reads it and asks for changes, but never keeps its own copy as the truth:
  - Settings and categories are TanStack Query queries (`useShellSettings`, `useCategories`).
  - The downloads come from `get_snapshot`, then from every `downloads-snapshot` event (`useSnapshotFeed`), into the `downloads` store.
- **The window owns its presentation state:** the current view, the category filter and the panel layout (the `ui` store, zustand). The layout is also saved, as below.
- **Questions to the user** go through one place: `confirmAction(options)` returns a promise, and `ConfirmHost` shows the dialog.

## Talking to Rust

- **Commands go through `api`** (`src/lib/api.ts`), never `invoke` directly:
  - Every answer is checked with its schema. An answer outside the contract is an `INTERNAL` error that says which command sent it.
  - A failed command becomes an `ApiError` with its code and the sentence for the user (`ERROR_MESSAGES`). A test keeps those sentences the same as Rust's (`error-codes.json`).
- **Events go through `events.ts`:**
  - Each payload is checked too. One that doesn't match is dropped, because the next snapshot brings the state again.
  - The window listens to three events: `downloads-snapshot`, `quit-requested` and `torrents-dropped`.
- **Permissions:** the web layer gets only Tauri's core app, event and window permissions (`src-tauri/capabilities/default.json`), and a test keeps it that way. The folder, `.torrent` and SSH key choosers and notifications are Rust's, through the app's own commands, so the page can't open a dialog or send a notification by itself. Files dropped on the window reach Rust too, which sends the page only the `.torrent` files among them.

## Starting up

`App` goes through these states, each shown explicitly:

1. **Not inside the desktop app** (no Tauri IPC): it says the window needs the desktop app.
2. **`app_info`:** a native side with another contract version is refused, saying so.
3. **`get_shell_settings`:** a failure shows its code, with "Try again".
4. **Setup not finished:** the first-run setup shows the transfer engine's state and asks for the download folder. `complete_onboarding` checks the folder: it must exist and can't be a link.
   - Unlike V1, the setup doesn't wait for the engine, because V2's window works without one and says why (#58).
   - The browser extension step comes with #64.
5. **The shell.**

## The shell

- **The layout:**
  - The navigation, a splitter, the current view and the status bar.
  - The navigation's width is a CSS variable, so a drag moves it without a React render. The splitter commits once, when the drag ends ([`Splitter`](../src/components/Splitter.tsx)).
  - `useLayoutPersistence` saves the layout 250 ms after the last change, through `set_appearance`. A failure shows as a toast.
  - A window too narrow for the navigation shows it as icons, without changing what is saved (`resolveLayout`).
- **The theme:**
  - `useTheme` puts the `dark` class on `<html>` for `dark`, or for `system` while the system is dark, and follows the system as it changes.
  - The colours are V1's design tokens, the PO's brand colours, in `src/index.css`. Tailwind reads them through `@theme`.
- **The views:**
  - The six lists show their counts in the navigation, and the download list below.
  - History (#32) and Settings (#30) are in the navigation's Library. Ctrl+, (⌘, on macOS) opens Settings, as in V1.
- **Quitting:**
  - When quitting would stop active downloads and the setting asks for it, Rust shows the window and sends `quit-requested`.
  - The window asks "Quit Velocity?". Only "Quit" calls `quit_app`, and "Keep downloading" changes nothing.

## Adding downloads

The Add dialog (#60, V1's) is in `src/features/add/`. Its state is in the `ui` store (`addDialog`), so anything can open it with `openAdd(prefill)`.

- **It opens from** the "Add download" button at the top of the navigation, Ctrl+N (⌘N on macOS), `.torrent` files dropped on the window (`useAddEntryPoints`), and a link the browser sends for the user to check (the `browser-request` event, #65).
- **Metalink files** (#222): "Choose Metalink files…" (`choose_metalinks`) puts them in the request's `metalinkFiles`, in place of links, as `.torrent` files do. It is disabled, with the reason, while the engine can't read Metalink (`availability`); the manager refuses one while Metalink is off (`PROTOCOL_DISABLED`). Each file's result is `metalinkAdded`, with every download it added.
- **What it checks as the user types** (`addLogic.ts`, V1's): each link's scheme and form, a login typed into a link, and a protocol the engine can't download (`get_capabilities`, with `availability`'s reason). Headers and the checksum are checked too. The native side checks everything again.
- **What it shows before adding:** the name the file gets (`naming.ts`), the automatic category and folder, a name already taken, and a missing chosen folder, which it offers to create (`check_destination`, 300 ms after the last change). A plain `ftp://` link gets the cleartext warning and "Require encryption (FTPS)".
- **What it shows after:** `add_downloads` answers for each link. All added closes the dialog with a toast. Otherwise it lists taken names ("Keep both (rename)"; a torrent's are only explained), duplicates ("Add anyway"; a torrent still in the list is only explained), and the links that failed with why (`outcome.ts`).
- **The SFTP host key** (`HostKeyPrompt`): when a download in the snapshot has failed with `error.hostKey`, the window asks once for that download and key. "Trust and continue" calls `trust_host_key`, then `retry_downloads`. A changed key is only explained.

## The download list

The list (#59, V1's) is in `src/features/downloads/`. It shows the snapshot as it is: what a download becomes after a command comes from the next snapshot, never from the window.

- **The table** (`DownloadTable`) is virtualised with TanStack Virtual, so only the rows in view are drawn. Its 13 columns are V1's (`columns.tsx`); the column chooser shows and hides them, always with Name.
- **Column widths:** dragging a column's edge previews the width and saves it once, when the drag ends; arrow keys change it by 16 px, and Home or a double-click restores it. The list's columns save through `set_appearance` over the latest saved appearance, so they never overwrite the layout ([SETTINGS.md](SETTINGS.md#the-window)).
- **Selection** (the `downloads` store, V1's rules): a click selects one, Ctrl or ⌘ toggles, Shift selects a range, and the checkboxes and select-all cover the rows shown. Arrow keys, Home, End, Ctrl+A and Escape move and select; Space pauses or resumes; Delete removes and Shift+Delete deletes the files. A download that leaves the snapshot leaves the selection.
- **Actions** (`useDownloadActions`): the toolbar and the right-click menu (`menus.tsx`) offer what `actionsFor` allows for the selection, as the manager allows it. Removing a running download and deleting files ask first. A command over several downloads reports what it couldn't do, from its `BatchResult`, and the buttons are disabled while a command is being sent. That a late status from the engine can't undo a pause or a cancel is the manager's rule ([ENGINE_CONTRACT.md](ENGINE_CONTRACT.md#the-managers-side)).
- **Statuses** (`statusLabel`): Retrying counts down to the engine's `retryAtMs`, and says "Waiting to retry" without one; a stalled direct download says "No data received"; one that came back without its login says "Needs its login". Speed and time left show only while bytes come, and the time left is the engine's speed over the bytes left. A notice says when the transfer engine isn't running, and when saved downloads are from a newer Velocity.
- **Entering a login again** (`LoginDialog`): resuming or retrying a download in the snapshot's `needsLogin` asks for its login, custom headers or SSH key, sends them with `set_download_login`, then resumes or retries it.
- **Show download:** the Add dialog's "Show download", for a duplicate already in the list, shows the whole list with that download selected (`showDownload`).
- **Opening:** a double-click on a finished download opens its file, and on any other shows its details. "Open file" and "Open folder" are in the right-click menu and the details panel. Opening a program asks first: the native side answers `CONFIRMATION_REQUIRED` until the user confirms (PO decision on #61).

## Torrents

The torrent screens (#63, V1's `src/features/torrents/`) are in `src/features/torrents/`.

- **The Add dialog** previews a single `.torrent` file (`preview_torrent_file`): its name, version, hash, a warning when it repeats a download, and its files as a tree (`FileTree`), each chosen by default. Several files are each added whole. For any torrent it offers the seeding policy (`SeedingFields`) and an upload limit.
- **Choose files** (`ChooseFilesDialog`) opens for a torrent waiting in Choose files: double-click it, or use its menu or its details.
- **Pasting** magnet or `.torrent` links outside a text field opens the Add dialog with them (`useAddEntryPoints`).
- **The rules** (`torrentLogic.ts`): the file tree, folder selection, the file choice to send (none when every file is chosen), the seeding policy in words, and a tracker's line.

## The details panel

The panel (#61, V1's `DetailsPanel`) is `src/features/downloads/DetailsPanel.tsx`.

- **What it shows:** everything comes from the live snapshot, so switching rows shows the new download at once, and no late answer can show another (PO decision on #61). There is no details command.
- **Which download:** while open, the panel follows the focused row (a click or the arrow keys). Without one it shows the download it was opened for, even while the search or a filter hides it, and says so. When its download leaves the list, it says that instead.
- **Opening it:** double-click a row, press Enter, use "Details" in the right-click menu, or the toolbar's panel button. Whether it is open, and its width, are saved with the layout (`appearance.layout.detailsVisible` and `detailsWidth`, [SETTINGS.md](SETTINGS.md#the-window)). The shell fits it into the window (`resolveLayout`) and passes the result through `ShellLayoutContext`.
- **Sections:**
  - **Transfer:** the speed, the average speed and the time left; a direct download's connections in use, how many it may use and what holds it back (`ConnectionLine`, #47); the next try while retrying; the verification state.
  - **Torrent** (`src/features/torrents/TorrentSection.tsx`, #61, #63): the peers and seeds connected, the bytes uploaded, the upload speed and the time seeding, as the torrent helper reports them ("Not reported yet" until it does); the info hashes, version and seeding policy (`get_torrent_details`); the trackers and the connected peers, asked for every second only while shown; re-announce, the upload limit, "Stop seeding", "Leave seeding queue", and changing its files, which keeps the files dropped unless the user asks to delete them.
  - **Source and destination**, and **Timeline**.
  - **Options for this download:** its category, its own speed limit, and a direct download's connections, which change while it runs (#47), so there is no restart to confirm.
- **Errors** show the engine's message, the HTTP status when there is one, whether retrying can help, and Retry when the download allows it.
- **Only what the engines report:** there are no per-connection or per-segment figures, because the engine doesn't report them.

## The history

The history (#32, V1's `HistoryView`) is `src/features/history/HistoryView.tsx`.

- **What it shows:** every finished download, newest first, 100 a page, with how it ended, why it failed, and whether it is still in the list.
- **Search and filter:** the search waits 250 ms after typing and goes back to the first page; the status filter is completed, failed or cancelled.
- **Removing:** "Remove selected" and "Clear history" (which asks first) take entries out of the history. Files are never touched, and a download still in the list stays there.
- **Again:** opens the Add dialog with the entry's link, so adding it goes through the usual questions (#60).

## Settings

The Settings screen (#30) is `src/features/settings/`: what each section holds and when it applies is in [SETTINGS.md](SETTINGS.md#the-settings-screen).

- **One query** (`useSettings`) holds what is saved. A change sends every section through `set_settings` and shows the answer; a refusal shows why, and the saved values again.
- **Numbers** are saved when the field is left or Enter is pressed, brought within their bounds (V1's).
- **Appearance** saves through `set_appearance`, like the panel widths.
- **About and diagnostics** is #28's `DiagnosticsPanel`.

## The native side of the window

These are in [`src-tauri/src/shell.rs`](../src-tauri/src/shell.rs) and [`tray.rs`](../src-tauri/src/tray.rs):

- **The tray:**
  - "Show Velocity", "Pause all", "Resume all" and "Quit". A left click shows the window.
  - Its tooltip says how many downloads are active, and their total speed.
  - On Linux the tray needs an AppIndicator library at runtime. Without a tray, closing the window quits and the app never starts hidden.
- **Closing and starting:**
  - Closing hides the window in the tray while `general.closeToTray` is on, as it is by default.
  - The window starts hidden (`tauri.conf.json`) and is shown at once, unless `general.startMinimized` is on and there is a tray.
  - A second launch shows the window.
- **Notifications:** `velocity_core::notices` compares each snapshot with the one before it:
  - a download completed, or started seeding;
  - a torrent waits for its files to be chosen;
  - a download failed, with why;
  - a download started;
  - the transfer engine or the torrent helper became unavailable.

  Each follows its setting ([SETTINGS.md](SETTINGS.md#the-window)). The first snapshot after starting notifies nothing.

## Testing

- **UI tests run in jsdom** (`vite.config.ts`, the `ui` project) with Testing Library. Only the Tauri boundary is mocked:
  - `src/test/fakeBackend.ts` answers the commands over Tauri's own `mockIPC`, with the generated contract fixtures, and records every call.
  - Events are sent with Tauri's `emit`.
- **Pure rules** (`layoutLogic`, `views`, `list`, `format`, `parseExtensions`) have unit tests.
- **The test setup gives elements a size** (`src/test/setup.ts`, as V1's), because jsdom does no layout and the virtualised list would draw no rows.
- **The layout in a real browser** is `pnpm test:e2e:layout` (L4, [TESTING.md](TESTING.md#the-test-pyramid)).
- **The tray and notifications** are checked by hand on each platform, and the PR records it.

## Not here yet

- **Owned by later issues:**
  - The file checks for opening, hardened further (#72).
  - The browser extension's steps (#64).
- **Keyboard shortcuts:** Ctrl+, opens Settings (#30), and Ctrl+N the Add dialog (#60).
- **Accessibility:** what is checked, and how, is in [ACCESSIBILITY.md](ACCESSIBILITY.md) (#66).
