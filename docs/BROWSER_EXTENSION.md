# Browser extension

The browser extension offers the browser's downloads to V2, and lets the browser carry on whenever V2 can't take one. It is V1's extension ported with V2's own identity (#64), so V1's and V2's extensions can be installed side by side. The native host it talks to, and registering it with each browser, are #65. The rules for this area are in `.claude/rules/browser-integration.md`.

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

To try a build: in Chrome, open `chrome://extensions`, turn on developer mode and load `browser-extension/dist/chrome` unpacked; in Firefox, open `about:debugging`, This Firefox, and load `browser-extension/dist/firefox/manifest.json` as a temporary add-on. It reaches V2 only once the native host is registered (#65).

## Tests

- **Unit tests** (`browser-extension/tests`, Vitest's `extension` project): interception, fallback, ask mode, the context menu, magnet links, the rules, the settings, the protocol and V2's identity, against fakes of the WebExtension APIs. `pnpm test:focused -- extension` runs them; `tsc --noEmit -p browser-extension` checks their types.
- **The manifest and the key** (`scripts/extension-lib.test.mjs`): permissions, pinned IDs and names.
- **Real browsers** (L4): Chromium and Firefox capturing a download into V2 with V1's extension installed too need the native host, so they come with #65.
