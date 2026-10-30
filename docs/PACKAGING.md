# Packaging

How V2 becomes an installer (#81). Windows is built; Linux packaging is #82 and macOS #83. Release-candidate builds and releases come later (#17, #18): nothing here publishes, signs or releases anything.

## The Windows installer

One installer, a per-user NSIS setup (`Velocity Download Manager V2_<version>_x64-setup.exe`), made by Tauri from [`src-tauri/tauri.release.conf.json`](../src-tauri/tauri.release.conf.json). No MSI (PO decision on #81).

- **For this user only:** it installs without an administrator prompt, into `%LOCALAPPDATA%\Velocity Download Manager V2`.
- **What it installs:** the app, and beside it the three programs the app starts: the transfer engine (`velocity-transfer.exe`), the torrent helper (`velocity-torrent.exe`) and the browser's native host (`velocity-native-host-v2.exe`). Also [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md) and the native libraries' `build-info.json` ([BUILDING_NATIVE.md](BUILDING_NATIVE.md#build-infojson)). No aria2 (ADR-0001): the build checks the installer's contents for it.
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

[`scripts/package-windows.mjs`](../scripts/package-windows.mjs) builds the three programs for release with `--features velocity-core/release-identity`, and stages them in `src-tauri/binaries/` under Tauri's sidecar names (`velocity-transfer-x86_64-pc-windows-msvc.exe`). It copies the notices and `build-info.json` to `src-tauri/staging/` (both folders are ignored by git), and runs `tauri build --config src-tauri/tauri.release.conf.json --features release-identity --bundles nsis`. The installer lands in `target/release/bundle/nsis/`, with its SHA-256 in a `.sha256` file beside it.

When 7-Zip is installed, the script then lists the installer and fails if a program or file is missing or anything named aria2 is in it. Without 7-Zip it says the check was NOT RUN; `--require-listing` makes that an error, as in CI.

**In CI:** the *Windows installer* workflow ([`package-windows.yml`](../.github/workflows/package-windows.yml)) runs the same command on a Windows runner and uploads the installer and its checksum as the `velocity-v2-windows-installer` artifact, kept 14 days. It is started by hand only, on the branch to package (PO decision on #81).

### Testing it beside V1

What #81 asks to record, on the PO's Windows PC with V1 installed:

1. **Install:** run the setup. Check the folder, the Start menu entry and the Apps & features entry say V2, and V1's are unchanged.
2. **Run:** start V2, add a download, and check V1 still starts and works.
3. **Upgrade:** install a newer V2 over it. This needs two builds with different versions; the upgrade from a release-candidate build waits for #17.
4. **Uninstall:** from Apps & features, without deleting the data. Check V2's folder, entry and browser keys are gone, its data folder is still there, and V1 still works.

### Signing

The installer isn't signed. Windows SmartScreen therefore warns on first run ("Windows protected your PC"); **More info**, then **Run anyway** starts it. Signing needs a code-signing certificate, which is a PO decision (procurement and cost), and Tauri's `bundle.windows.certificateThumbprint` or `signCommand` set for the release workflow (#18). No production key is used anywhere today.
