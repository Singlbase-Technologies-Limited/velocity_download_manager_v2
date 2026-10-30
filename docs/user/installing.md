# Installing

V2 installs beside the original Velocity Download Manager (V1): each has its own programs, menu entries and data, and installing or removing one never changes the other ([V1 and V2](v1-and-v2.md)).

**No signed release yet:** V2's installers are test builds, made by its CI and not signed.

## Windows

The installer is `Velocity Download Manager V2_<version>_x64-setup.exe`.

- **For your user only:** it installs without an administrator prompt, into `%LOCALAPPDATA%\Velocity Download Manager V2`, with a Start menu entry and an Apps & features entry, all named "Velocity Download Manager V2".
- **Unsigned:** Windows SmartScreen warns the first time ("Windows protected your PC"). Choose **More info**, then **Run anyway**.
- **WebView2:** if the PC lacks Microsoft's WebView2, the installer downloads it. Windows 10 and 11 normally have it.
- **Your data** is kept in `%APPDATA%\com.velocity.downloadmanager.v2`, apart from V1's.
- **Third-party licences** are listed in `THIRD_PARTY_NOTICES.md` in the install folder.

## Linux

Two packages, for 64-bit PCs:

| Package | For |
|---|---|
| `.deb` | Ubuntu 22.04 and newer, Debian 12 and newer, and distributions based on them |
| `.rpm` | Fedora, openSUSE and similar distributions of the same age or newer |

Install it with your package manager, which also installs the WebKitGTK, GTK and AppIndicator libraries V2 needs:

```sh
sudo apt install "./Velocity Download Manager V2_<version>_amd64.deb"   # Ubuntu, Debian
sudo dnf install "./Velocity Download Manager V2-<version>-1.x86_64.rpm" # Fedora
```

Use the file name of the package you have, in quotes because it has spaces. A release candidate's files have other names, such as `velocity-v2-0.1.0-rc.1-<commit>-<date>-linux-amd64.deb`. V2 then appears in your applications menu as "Velocity Download Manager V2". Its data is kept in `~/.local/share/com.velocity.downloadmanager.v2`, apart from V1's, and the third-party licences are in `/usr/lib/Velocity Download Manager V2/THIRD_PARTY_NOTICES.md`.

No AppImage is made for now.

## Upgrading

Install a newer version over the old one: run the newer Windows installer, or install the newer Linux package. Your downloads, settings, history and browser registrations are kept.

## Removing V2

- **Windows:** remove "Velocity Download Manager V2" in Apps & features. That removes the program and V2's browser registrations. Your downloads, settings and history stay unless you tick **Delete the application data**.
- **Linux:** `sudo apt remove velocity-download-manager-v2`, or `sudo dnf remove velocity-download-manager-v2`. That removes the programs, the menu entry and the icons. Your downloads, settings and history stay in your home folder. **First unregister the browsers** in Settings, Browser: a package can't reach the registrations in your home folder, so they would stay, pointing to a program that is gone.

Downloaded files are never removed, on either system. Removing V2 never changes V1.
