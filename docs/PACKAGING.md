# Packaging

How V2 becomes an installer: Windows (#81) and Linux (#82) are built; macOS is #83. Release-candidate builds and releases come later (#17, #18): nothing here publishes, signs or releases anything.

## The Windows installer

One installer, a per-user NSIS setup (`Velocity Download Manager V2_<version>_x64-setup.exe`), made by Tauri from [`src-tauri/tauri.release.conf.json`](../src-tauri/tauri.release.conf.json). No MSI (PO decision on #81).

- **For this user only:** it installs without an administrator prompt, into `%LOCALAPPDATA%\Velocity Download Manager V2`.
- **What it installs:** the app, and beside it the three programs the app starts: the transfer engine (`velocity-transfer.exe`), the torrent helper (`velocity-torrent.exe`) and the browser's native host (`velocity-native-host-v2.exe`). Also V2's [`LICENSE`](../LICENSE) (GPL-3.0 requires giving it with every copy, ADR-0017), [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md) and the native libraries' `build-info.json` ([BUILDING_NATIVE.md](BUILDING_NATIVE.md#build-infojson)). No aria2 (ADR-0001): the build checks the installer's contents for it.
- **The release identity:** every program in it is built with the `release-identity` feature, so it runs as `com.velocity.downloadmanager.v2`, with its data in `%APPDATA%\com.velocity.downloadmanager.v2`. A development build never uses that folder ([COEXISTENCE.md](COEXISTENCE.md)).
- **WebView2:** the installer downloads Microsoft's WebView2 bootstrapper if the machine lacks WebView2, as Tauri does by default. Windows 10 and 11 normally have it already.

### Beside V1

V1 is installed as "Velocity Download Manager". V2's installer uses its own product name, so nothing it writes has V1's name ([ADR-0016](decisions/0016-release-product-name-v2.md)):

| | V2 | V1 |
|---|---|---|
| Install folder | `%LOCALAPPDATA%\Velocity Download Manager V2` | `%LOCALAPPDATA%\Velocity Download Manager` |
| Apps & features entry (`HKCU\…\Uninstall\<name>`) | `Velocity Download Manager V2` | `Velocity Download Manager` |
| Data folder | `%APPDATA%\com.velocity.downloadmanager.v2` | `%APPDATA%\com.velocity.downloadmanager` |
| Browser registrations | `com.velocity.download_manager_v2` | `com.velocity.download_manager` |

Both use the publisher name "Velocity Project". The installer keeps its own settings under `HKCU\Software\Velocity Project\Velocity Download Manager V2`, and removes the shared `Velocity Project` key only when it is empty, so V1's stays.

Offering to remove V1 from V2's installer is left for a later issue (PO decision on #81).

### Uninstalling and upgrading

- **Uninstalling** removes the programs, the shortcut and the Apps & features entry. It also removes V2's browser registrations: the `HKCU\Software\<browser>\NativeMessagingHosts\com.velocity.download_manager_v2` keys for Chrome, Chromium, Edge, Brave and Firefox ([`src-tauri/windows/installer-hooks.nsh`](../src-tauri/windows/installer-hooks.nsh)). V1's keys have another name and stay.
- **Your downloads and settings stay** unless you tick "Delete the application data", which removes `%APPDATA%\com.velocity.downloadmanager.v2` and `%LOCALAPPDATA%\com.velocity.downloadmanager.v2`. Downloaded files are never removed.
- **Upgrading:** a newer installer runs the old version's uninstaller with `/UPDATE`. That keeps the data and the browser registrations, so the browsers keep working.

### Building it

On Windows, after `pnpm native:build`:

```sh
pnpm package:windows
```

[`scripts/package-windows.mjs`](../scripts/package-windows.mjs), with the steps it shares with the Linux packages in [`scripts/package-build.mjs`](../scripts/package-build.mjs), builds the three programs for release with `--features velocity-core/release-identity`, and stages them in `src-tauri/binaries/` under Tauri's sidecar names (`velocity-transfer-x86_64-pc-windows-msvc.exe`). It copies the notices and `build-info.json` to `src-tauri/staging/` (both folders are ignored by git), and runs `tauri build --config src-tauri/tauri.release.conf.json --features release-identity --bundles nsis`. The installer lands in `target/release/bundle/nsis/`, with its SHA-256 in a `.sha256` file beside it.

It also writes the platform's records for a release candidate in `target/release/bundle/`: `native-build-info-windows.json` and `libtorrent-info-windows.json` (the native build records) and `licence-report-windows.json` ([LICENSING.md](LICENSING.md#third-party-notices)); the Linux script writes the `-linux` ones. When 7-Zip is installed, the script then lists the installer and fails if a program or file is missing or anything named aria2 is in it. Without 7-Zip it says the check was NOT RUN; `--require-listing` makes that an error, as in CI.

**In CI:** the *Windows installer* workflow ([`package-windows.yml`](../.github/workflows/package-windows.yml)) runs the same command on a Windows runner and uploads the installer and its checksum as the `velocity-v2-windows-installer` artifact, kept 14 days. It is started by hand only, on the branch to package (PO decision on #81).

### Testing it beside V1

What #81 asks to record, on the PO's Windows PC with V1 installed:

1. **Install:** run the setup. Check the folder, the Start menu entry and the Apps & features entry say V2, and V1's are unchanged.
2. **Run:** start V2, add a download, and check V1 still starts and works.
3. **Upgrade:** install a newer V2 over it. This needs two builds with different versions; the upgrade from a release-candidate build waits for #17.
4. **Uninstall:** from Apps & features, without deleting the data. Check V2's folder, entry and browser keys are gone, its data folder is still there, and V1 still works.

### Signing

The installer isn't signed. Windows SmartScreen therefore warns on first run ("Windows protected your PC"); **More info**, then **Run anyway** starts it. Signing needs a code-signing certificate, which is a PO decision (procurement and cost), and Tauri's `bundle.windows.certificateThumbprint` or `signCommand` set for the release workflow (#18). No production key is used anywhere today.

## The Linux packages

A deb, for Ubuntu and Debian, and an rpm, for Fedora and openSUSE, made by Tauri from the same [`src-tauri/tauri.release.conf.json`](../src-tauri/tauri.release.conf.json). No AppImage: it would carry its own copies of WebKitGTK, GTK and their libraries, several of them LGPL, which needs a licence review of its own (PO decision on #82; [LICENSING.md](LICENSING.md)).

- **Which Linux:** they are built on Ubuntu 22.04, so they need glibc 2.35 at most, and run on Ubuntu 22.04 and newer, Debian 12 and other distributions of the same age or newer (PO decision on #82). The build checks every program against that limit.
- **Package name:** `velocity-download-manager-v2`, version 0.1.0 (release 1 in the rpm). The files are `Velocity Download Manager V2_<version>_amd64.deb` and `Velocity Download Manager V2-<version>-1.x86_64.rpm`, each about 17 MiB.
- **What they install:**

  | Path | What |
  |---|---|
  | `/usr/bin/velocity-desktop-v2` | The app |
  | `/usr/bin/velocity-transfer`, `/usr/bin/velocity-torrent`, `/usr/bin/velocity-native-host-v2` | The transfer engine, the torrent helper and the browser's native host, beside the app as it expects |
  | `/usr/lib/Velocity Download Manager V2/` | `LICENSE`, `THIRD_PARTY_NOTICES.md` and the native libraries' `build-info.json` |
  | `/usr/share/applications/Velocity Download Manager V2.desktop` | The menu entry |
  | `/usr/share/icons/hicolor/<size>/apps/velocity-desktop-v2.png` | The icons |

- **What they depend on:** the distribution's WebKitGTK 4.1, GTK 3 and Ayatana AppIndicator, for the tray. Tauri adds them: the deb `Depends:` on `libayatana-appindicator3-1, libwebkit2gtk-4.1-0, libgtk-3-0`, and the rpm `Requires:` `libayatana-appindicator3.so.1`, `libgtk-3.so.0` and `libwebkit2gtk-4.1.so.0`. The package manager installs them; nothing of theirs is bundled.
- **What they don't depend on:** libcurl, OpenSSL, nghttp2, zlib, libtorrent and Boost are linked into V2's programs statically ([BUILDING_NATIVE.md](BUILDING_NATIVE.md)), so a distribution's copies are never used. The build checks that no program loads one.
- **The release identity:** as on Windows, every program runs as `com.velocity.downloadmanager.v2`, with its data in `~/.local/share/com.velocity.downloadmanager.v2` ([COEXISTENCE.md](COEXISTENCE.md)).

### Beside V1

V1's Linux packages are named `velocity-download-manager`, and install `/usr/bin/velocity-desktop`, `/usr/bin/velocity-aria2c`, `/usr/bin/velocity-native-host`, `/usr/lib/Velocity Download Manager/`, `Velocity Download Manager.desktop` and `velocity-desktop.png` icons (V1's `tauri.conf.json` at `e9d717a8`). V2's packages share none of these, so both install side by side; the build fails if a V2 package would install any of them.

### Removing and upgrading

- **Removing** the package (`sudo apt remove velocity-download-manager-v2`, or `sudo dnf remove velocity-download-manager-v2`) removes the programs, the notices, the menu entry and the icons.
- **Your downloads and settings stay** in your home folder, and downloaded files are never removed.
- **Browser registrations stay:** they are files in each user's home folder, which a package can't reach. Unregister the browsers in Settings, Browser, before removing V2. A registration left behind points to a program that's gone, so the browser reports the native host as missing; V1's registrations have another name and aren't touched.
- **Upgrading:** install a newer package over it. The package manager replaces the files; the data and the browser registrations stay.

### Building them

On Linux, after `pnpm native:build`:

```sh
pnpm package:linux
```

[`scripts/package-linux.mjs`](../scripts/package-linux.mjs) builds and stages the programs and files as `package:windows` does, and runs `tauri build --config src-tauri/tauri.release.conf.json --features release-identity --bundles deb,rpm`. The packages land in `target/release/bundle/deb/` and `target/release/bundle/rpm/`, each with its SHA-256 in a `.sha256` file beside it. Then it checks:

- **each package's files** (`dpkg-deb -c`, `rpm -qlp`): the app, its three programs, the notices, `build-info.json` and the menu entry are there, nothing is named aria2, and no path is one of V1's;
- **the programs in the deb** (`readelf -d`, `objdump -T`): none loads a system copy of a library V2 links statically, and none needs a glibc newer than 2.35.

A check whose tool is missing is reported NOT RUN; `--require-listing` makes that an error, as in CI. Built on a newer distribution than Ubuntu 22.04, the glibc check fails, as it should.

**In CI:** the *Linux packages* workflow ([`package-linux.yml`](../.github/workflows/package-linux.yml)) runs the same command on an Ubuntu 22.04 runner, with caches of its own ([CI.md](CI.md#caching)), and uploads the deb, the rpm and their checksums as the `velocity-v2-linux-packages` artifact, kept 14 days. Then [`scripts/package-install-test.sh`](../scripts/package-install-test.sh) installs each package in a clean Ubuntu 22.04, Ubuntu 24.04 and Debian 12 container (the deb) and Fedora 42 container (the rpm), checks that every file is there, that every program finds its libraries and that the engine and torrent helper start, then removes the package and checks its files are gone. It is started by hand only, on the branch to package.

### Testing them beside V1

What #82 asks to record, on Ubuntu 24.04 with V1's package installed: install V2's deb, start V2 and add a download, check V1 still starts and works, then remove V2's package and check V1 is unchanged.
