# Browser integration

V2's browser extension sends downloads from your browser to V2. It works with Chrome, Edge and Brave (Chrome 116 or later) and Firefox (115 or later).

## Beside V1

V2's extension is its own: it is named **Velocity Download Manager V2**, its menu item is **Download with Velocity V2**, and it talks only to V2. V1's extension can stay installed at the same time; each sends links only to its own app, and registering or unregistering V2 never touches V1's.

## Installing the extension

**Not in the stores yet:** the extension isn't in the Chrome Web Store or on addons.mozilla.org, and the installers don't set it up for you. For now it is loaded by hand from a build:

- **Chrome, Edge, Brave:** open `chrome://extensions` (or `edge://extensions`, `brave://extensions`), turn on developer mode, choose **Load unpacked**, and pick the extension's `chrome` folder.
- **Firefox:** open `about:debugging`, **This Firefox**, **Load Temporary Add-on**, and pick the `manifest.json` in the extension's `firefox` folder. Firefox removes a temporary add-on when it closes.

How to build the extension is in the [developer documents](../BROWSER_EXTENSION.md#building).

## Connecting it to V2

In V2, open **Settings, Browser**. Each browser (Chrome, Chromium, Edge, Brave, Firefox) has **Register** or **Unregister**. Registering is for your user only and needs no administrator rights. The first-run setup offers the same.

"Registered for another copy" means the registration points to a V2 that was moved or reinstalled elsewhere: **Register** again fixes it.

**Before removing V2 on Linux,** unregister the browsers: a package can't remove registrations from your home folder ([Installing](installing.md#removing-v2)).

## What the extension does

- **It takes over matching downloads:** by default, archives, disk images, installers, video, audio, PDF and e-books, and files of 20 MiB or more when the browser knows the size. The browser's own copy is cancelled once V2 has the link.
- **Modes,** in the extension's popup: send matching downloads to V2 (the default), ask first, or leave every download to the browser.
- **Rules,** in the extension's options: the file types, the size, sites to always or never send, and private windows (off by default).
- **The right-click menu** on links, images, video and audio: **Download with Velocity V2**.
- **Magnet links and `.torrent` downloads:** off by default. Turning magnet links on asks for access to pages. V2 always asks before adding a torrent.
- **What it never sends:** cookies, page content or logins. A download that needs them may fail in V2; the browser can then download it.

## What V2 does with a link

Settings, Browser has two switches, both on by default:

- **Ask before adding:** the Add dialog opens, filled in, and V2's window comes forward. Off, the link is added at once to the automatic folder. A taken name, a link added before, a torrent or a magnet link still opens the dialog.
- **Take links from the browser:** off, V2 refuses every link, and the browser downloads it itself.

**A link to this computer or your local network** (such as `127.0.0.1`, `192.168.x.x` or `10.x.x.x`) waits in the list before anything is sent: **Download anyway** in its details panel goes on. A link you type or paste yourself isn't held.

## When V2 can't take a link

If V2 isn't running, isn't connected, or refuses the link, the browser downloads the file itself. The extension's badge turns red with "!", and its popup says why:

- **"Velocity V2 is not running":** start V2.
- **"The connection to Velocity V2 is not set up":** register that browser in V2's Settings, Browser, then reload the extension.
