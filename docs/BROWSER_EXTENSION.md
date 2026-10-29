# Browser extension

The browser extension offers the browser's downloads to V2, and lets the browser carry on whenever V2 can't take one. It is V1's extension ported with V2's own identity (#64), so V1's and V2's extensions can be installed side by side. It reaches V2 through V2's own native host, which each browser starts once it is registered in Settings, Browser (#65). The rules for this area are in `.claude/rules/browser-integration.md`.

## What it does

- **Interception:** when the browser starts a download that matches the rules, the extension pauses it and offers it to V2. If V2 takes it, the browser's copy is cancelled and erased; on any failure the browser's download resumes.
- **Modes:** send matching downloads to V2 (the default), ask first in a small window, or leave every download to the browser. Closing the ask window lets the browser continue.
- **Rules:** file types (archives, disk images, installers, video, audio, PDF and e-books by default), a size threshold (20 MiB by default; only when the browser knows the size), allow and deny lists of domains with their subdomains, and private windows (off by default).
- **Never intercepted:** `blob:`, `data:`, `file:` and other browser-only links; downloads the extension started itself as a fallback; and the same link twice within 5 seconds.
- **Context menu:** "Download with Velocity V2" on links, images, video and audio. If V2 can't take the link, the browser downloads it.
- **Popup:** shows whether V2 is reachable, switches the mode, and sends a typed link.
- **Magnet links and `.torrent` downloads:** off by default. Turning them on asks for access to pages, which magnet clicks need; `.torrent` downloads work without it. V2 always asks before adding a torrent.
- **Failures:** the toolbar badge turns red with "!", and its tooltip says what went wrong, for example that the connection isn't set up or that V2 didn't answer within 10 seconds.
- **What it never sends:** cookies, page content or logins. A download that needs them may fail in V2, and the browser can then download it.

## Identity

V2's extension uses none of V1's names or IDs ([ADR-0003](decisions/0003-v2-app-identity.md), [COEXISTENCE.md](COEXISTENCE.md)). All of them come from `branding.json`.

| | V2 development build | V2 release build | V1 |
|---|---|---|---|
| Name | Velocity Download Manager V2 (dev) | Velocity Download Manager V2 | Velocity Download Manager |
| Native host it talks to | `dev.velocity.download_manager_v2` | `com.velocity.download_manager_v2` | `com.velocity.download_manager` |
| Chrome extension ID | `adhefdmekgeafmopfbiappajaolcbhkc` | the same | `eecopkgfiffjgmngfmjmdaejbkbbippo` |
| Firefox add-on ID | `velocity-download-manager-v2@velocity.local` | the same | `velocity-download-manager@velocity.local` |

- **Telling them apart:** V2's name, toolbar title, context-menu item and messages all say "Velocity V2".
- **The Chrome key:** the manifest's `key` is the public half of a key pair generated for V2 in #64, and it pins the Chrome extension ID. The private half is held by the Product Owner, outside the repository; the repository ignores `*.pem` and `*.key` files. A test derives the ID from the key and checks it against `branding.json` (`scripts/extension-lib.test.mjs`).
- **Store publication** isn't part of #64.

## Protocol

The extension speaks V1's native messaging protocol, version 1, unchanged: it can ping, ask for the app's status, or offer a link (`download.add` with the URL and, when known, the referrer, the file name, the size, the MIME type and where it came from). It never sends paths, commands or engine options. `tests/fixtures/contract/browser-messages.json` holds valid and invalid messages; the extension's tests check its messages against it, and the native host (#65) accepts the valid ones and refuses the invalid ones.

## Permissions

The same as V1's: `downloads`, `nativeMessaging`, `storage`, `contextMenus` and `scripting`. Access to pages (`<all_urls>`) is optional, and asked for only when magnet capture is turned on. The extension's pages run only the extension's own scripts (`script-src 'self'`).

## Building

```sh
pnpm build:extension              # development build: talks to the development app
pnpm build:extension -- --release # release build: talks to the release app
```

Both write `browser-extension/dist/chrome` (Chrome, Edge and Brave, Manifest V3 with a service worker; Chrome 116 or later) and `browser-extension/dist/firefox` (Manifest V3 with a background script; Firefox 115 or later). Each script is bundled by Vite as a classic script, and the icons are the desktop app's own (`src-tauri/icons`).

To try a build: in Chrome, open `chrome://extensions`, turn on developer mode and load `browser-extension/dist/chrome` unpacked; in Firefox, open `about:debugging`, This Firefox, and load `browser-extension/dist/firefox/manifest.json` as a temporary add-on. It reaches V2 once the native host is registered with that browser ([below](#registering-with-each-browser)).

## Tests

- **Unit tests** (`browser-extension/tests`, Vitest's `extension` project): interception, fallback, ask mode, the context menu, magnet links, the rules, the settings, the protocol and V2's identity, against fakes of the WebExtension APIs. `pnpm test:focused -- extension` runs them; `tsc --noEmit -p browser-extension` checks their types.
- **The manifest and the key** (`scripts/extension-lib.test.mjs`): permissions, pinned IDs and names.
- **The native host** (`crates/velocity-native-host/tests/host_e2e.rs`, L2): the real host program, with browser framing, against a real local listener: callers other than V2's extensions (V1's included) refused, only valid messages passed on with the token and the caller, a wrong token refused, the app not running, an oversized message.
- **The bridge and registration** (`velocity_core::browser`, unit tests): the protocol against the shared fixture, the token and its file, the local connection and its limits, the routing (callers, settings, torrents always asking, rate limit, what adding did), and registering with each browser in a folder and a registry key of the test's own, with V1's manifests and registry values in place and unchanged afterwards.
- **Real browsers** (L4): `pnpm test:e2e:chromium` and `pnpm test:e2e:firefox` (`tests/e2e/browser`). A link clicked on a local page is intercepted by V2's extension, goes through V2's native host into a running V2, which downloads it; the browser's copy is cancelled. V1's extension is installed too, with a recording stand-in for V1's host: the test checks V1's extension reached only V1's host and V2 recorded the download once.
  - **Build first:** `cargo build -p velocity-desktop-v2 -p velocity-transfer -p velocity-torrent -p velocity-native-host` and `pnpm build:extension`, and V1's extension built in `../velocity-download-manager` (or `V1_EXTENSION_DIR`). No V2 development instance may be running, since V2 is single-instance.
  - **Chromium** runs on Windows and Linux. On Windows, Chromium reads native hosts only from the registry: the test writes `HKCU\Software\Chromium\NativeMessagingHosts` keys for V2's development host and V1's stand-in, and puts back any key that was there.
  - **Firefox** runs on Linux only (PO decision on #64), with Firefox, geckodriver and `zip` installed, as V1's did.

## The native host

`velocity-native-host-v2` (`crates/velocity-native-host`) is what the browser starts for V2's extension. It is ported from V1's host with V2's names.

- **Framing:** native messaging's: a 32-bit length in native byte order, then UTF-8 JSON. A message over 64 KiB is refused before it is read; an answer over 1 MB is replaced by an error.
- **Callers:** only V2's Chrome extension origin and Firefox add-on ID, from the arguments the browser passes. V1's extensions are refused (`FORBIDDEN_CALLER`).
- **Checked twice:** the host checks each message (`velocity_core::browser::protocol`), and the app checks it again.
- **The local connection:** the host passes a valid message to the running app over V2's own pipe or socket ([COEXISTENCE.md](COEXISTENCE.md)), with this launch's token, which the app writes to `<data folder>/ipc.token` when it starts (32 random bytes from the operating system, `getrandom`). On Windows the pipe is created first and refuses remote clients; on Unix the socket and the token are readable by this user only. A wrong token is refused (`UNAUTHORIZED`); without a running app the browser hears `DESKTOP_NOT_RUNNING`.
- **Nothing else:** it reads the token and nothing more, runs nothing, and has no other way into the app.

## What the app does with a link

`velocity_core::browser::bridge`, ported from V1's bridge:

- **At most 30 messages in 10 seconds;** more are refused (`RATE_LIMITED`).
- **Settings, Browser:** "Take links from the browser" (on) and "Ask before adding" (on), V1's defaults. With links refused, the extension's popup says so and the browser downloads.
- **Asking:** the link opens the Add dialog, filled in with what the browser sent, and the window comes forward. Torrents and magnet links always ask, whatever the setting.
- **Adding at once:** with "Ask before adding" off, the link is added to the automatic folder, with the browser's file name when it is a safe one. A taken name or a link added before opens the Add dialog instead. Either way the download is marked as from the browser, so a link that reaches this computer or a private network waits for the user ([SECURITY.md](SECURITY.md#links-redirects-and-local-addresses-67)). A link typed in place of the browser's in the dialog isn't from the browser.

## Registering with each browser

Settings, Browser lists Chrome, Chromium, Edge, Brave and Firefox, each with Register or Unregister (`register_browser`, `unregister_browser`). Registering is for this user only: nothing system-wide, no elevation.

- **The manifest** names V2's host, the host program beside the app, and the extensions allowed to start it: V2's Chrome extension (`allowed_origins`) or Firefox add-on (`allowed_extensions`).
- **Where:** on Linux, each browser's own folder (for example `~/.config/google-chrome/NativeMessagingHosts`, `~/.mozilla/native-messaging-hosts`); on Windows, `<data folder>/native-messaging/<host>.<browser>.json`, with each browser's `HKEY_CURRENT_USER\Software\…\NativeMessagingHosts\<host>` key pointing to it (`windows-sys`, PO decision on #65). The macOS places are V1's, untested (#83).
- **V1 is never touched:** every file and key is named after V2's host. Unregistering removes V2's key, and V2's manifest only when the file there names V2's host and is a plain file, never a link.
- **A moved app:** a manifest naming another copy of the host shows "Registered for another copy"; registering again fixes it.
