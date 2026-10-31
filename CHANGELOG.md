# Changelog

What changed in each version of Velocity Download Manager V2. Versions follow [Semantic Versioning](https://semver.org/); while V2 is below 1.0.0, a minor version may change things that break. How sections are drafted from the merged pull requests, and edited into release notes, is in [docs/RELEASE_PROCESS.md](docs/RELEASE_PROCESS.md).

## Unreleased

Nothing yet.

## 0.1.0 - 2026-10-31

The first version of Velocity Download Manager V2: a new app, not an update of the original Velocity Download Manager (V1). V2 has its own download engine, on libcurl, and uses libtorrent for torrents. It installs and runs beside V1, and shares nothing with it ([V1 and V2](docs/user/v1-and-v2.md)).

### Downloads

- **Protocols:** HTTP, HTTPS, FTP, FTPS (explicit and implicit) and SFTP, with BitTorrent and Metalink ([Protocols and limits](docs/user/protocols.md)).
- **Fast by default:** a download starts with every connection its performance profile allows, then keeps only those that make it faster, and changes its connections while it runs, without restarting.
- **Resume:** pause and resume, and a download continues after a crash, a restart or a full disk without losing what it already had. If the file changed on the server, V2 notices and starts again rather than mixing two versions.
- **Checksums:** a download can be checked against a checksum, and one that can't be verified is never shown as verified.
- **Mirrors and Metalink:** several sources for one file, with failover when one stops working. Metalink files can be added from a link or from your computer; Metalink is off by default, as in V1.
- **Queue and limits:** how many download at once, their order, a schedule window, a global speed limit and one for each download.
- **Categories** with automatic folders. V2 never replaces an existing file: it asks about a taken name or renames the new file.
- **Proxy:** new in V2. HTTP, HTTPS or SOCKS5, for every download, torrents included.
- **Logins:** remembered in the system keychain only when you tick it, and never written to V2's database, logs or diagnostics.

### Torrents

- `.torrent` files and links and magnet links, with v1, v2 and hybrid torrents. A magnet is checked before anything downloads, and you choose the files.
- Seeding with limits and a seeding queue, trackers with re-announce, peers, DHT, peer exchange and local peer discovery, upload and download limits, and a listening port (6890) with optional port mapping.
- Torrents resume after a restart or a crash, and removing one deletes only its own files.

### The app

- The download list, a details panel (connections, errors, torrent activity, options), the History view, Categories and Settings.
- First-run setup, a tray icon with close to tray, notifications, and light and dark themes.
- Logs with secrets removed, a diagnostics export, and local crash logs. V2 sends nothing anywhere except the downloads themselves.
- Accessibility: keyboard focus returns where it was after a dialog, and text contrast meets WCAG AA.

### Safer by default

- V2 asks before following a redirect from `https://` to `http://`, before downloading a browser link to this computer or your local network, and before trusting an SFTP server's key.
- TLS certificates are always verified.
- V2 deletes only files its downloads made, and never follows a link when it removes one.

### Browser integration

- V2's own extension for Chrome, Edge, Brave and Firefox sends the browser's downloads to V2, with modes, rules and a right-click menu. It works beside V1's extension.
- Settings, Browser connects V2 to each browser, for your user only.
- **Not in the stores yet:** for this version, the extension is loaded by hand ([Browser integration](docs/user/browser.md#installing-the-extension)). Publishing it in the Chrome Web Store, Edge Add-ons and Firefox Add-ons follows (#240, #241).

### Installing

- **Windows, 64-bit:** an installer for your user only, with no administrator prompt. Uninstalling removes V2's browser connections; your downloads, settings and history stay unless you choose to delete them.
- **Linux, 64-bit:** a `.deb` for Ubuntu 22.04 and newer and Debian 12 and newer, and an `.rpm` for Fedora and similar. Before removing V2 on Linux, disconnect the browsers in Settings, Browser.
- **Unsigned:** the installers aren't code-signed yet, so Windows SmartScreen warns the first time ([Installing](docs/user/installing.md)).
- **macOS** isn't supported yet.
- V2 is free software under the GNU General Public License, version 3 only.

### Coming from V1

- **Nothing is imported from V1:** V2 starts with no downloads, its own settings and an empty history. Finish unfinished V1 downloads in V1, or add their links to V2 again; their partial data is aria2's and can't be continued.
- **Settings that changed:** "allow mid-download tuning", the progress polling interval, the Custom profile's "split" count, the torrent-engine choice and "look for more peers below this speed" are gone, because V2 does these by itself. File allocation and disk cache are now Preallocation and the write buffer, in Settings, Advanced ([V1 and V2](docs/user/v1-and-v2.md#what-is-different-in-v2)).

### Known limits

- FTP and SFTP download over one connection.
- SFTP and torrents can't use an HTTPS proxy; choose HTTP or SOCKS5 for them.
- A private torrent finds no peers after V2 restarts, because V2 doesn't keep tracker addresses, which can hold passkeys. Add it again from its `.torrent` file or magnet link.
