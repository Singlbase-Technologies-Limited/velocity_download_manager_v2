# The UI

How the window (`src/`) is built: where its state lives, how it talks to the native side, and how it is styled and tested. The commands and types it uses are in [CONTRACTS.md](CONTRACTS.md), and the settings they save are in [SETTINGS.md](SETTINGS.md). The shell came with #29, as a port of V1's ([PARITY.md](PARITY.md)).

## The parts

| Folder | What it holds |
|---|---|
| `src/app/` | The window as a whole. `App` handles starting up, `Shell` the window once it is set up, plus `Sidebar`, `StatusBar`, the theme hook and the error screen |
| `src/features/` | One folder for each screen: `onboarding/` (the first-run setup), `categories/` (the Categories screen and its editor), and `downloads/` (where the list goes, #59) |
| `src/components/` | Shared pieces, one per file: `Button`, `Dialog`, `ConfirmDialog` and `confirmAction`, `Field`, `Input`, `Splitter`, `EmptyState`, `Spinner`, `Badge`, `BrandIcon` and `CategoryGlyph` |
| `src/layout/` | The panel rules (`layoutLogic.ts`, V1's), and saving the layout (`useLayout.ts`) |
| `src/lib/` | The bridge to Rust (`api.ts`, `events.ts`, `errors.ts`), the queries (`queries.ts`), and `format.ts` and `cn.ts` |
| `src/stores/` | Presentation state (`ui.ts`), and the latest snapshot of the downloads (`downloads.ts`) |
| `src/downloads/` | Rules about downloads that more than one part uses, such as the counts for each list (`views.ts`) |
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
  - The window listens to two events: `downloads-snapshot` and `quit-requested`.
- **Permissions:** the web layer gets only Tauri's core app, event and window permissions (`src-tauri/capabilities/default.json`), and a test keeps it that way. The folder chooser and notifications are Rust's, through the app's own commands, so the page can't open a dialog or send a notification by itself.

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
  - The six lists show their counts in the navigation. The list itself is #59's, and says so.
  - History and Settings come with #32 and #30.
- **Quitting:**
  - When quitting would stop active downloads and the setting asks for it, Rust shows the window and sends `quit-requested`.
  - The window asks "Quit Velocity?". Only "Quit" calls `quit_app`, and "Keep downloading" changes nothing.

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
- **Pure rules** (`layoutLogic`, `views`, `format`, `parseExtensions`) have unit tests.
- **The tray and notifications** are checked by hand on each platform, and the PR records it.

## Not here yet

- **Owned by later issues:**
  - The download list, its details panel and its columns (#59, #61).
  - The Add dialog (#60).
  - Settings, including the theme, density, tray and notification switches (#30).
  - History (#32).
  - The browser extension's steps (#64).
- **Keyboard shortcuts:** V1's Ctrl+N and Ctrl+, come with the screens they open.
- **The accessibility audit** is #66.
