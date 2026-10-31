# Release candidates

A release candidate is a set of installers built from one verified commit on `main`, for the PO to install and test before a release (#17). It isn't a release: nothing is signed, tagged or published by it. Releases are #18; versions, tags and the changelog are #84.

## What one holds

The *Release candidate* workflow ([`release-candidate.yml`](../.github/workflows/release-candidate.yml)) uploads one artifact, named like its files, `velocity-v2-<version>-<candidate>-<short commit>-<YYYYMMDD>`, where `<candidate>` is `rc.N` for a tag and `main` for a hand-started run. For example `velocity-v2-0.1.0-rc.1-3a93690-20261031`:

| File | What |
|---|---|
| `…-windows-x64-setup.exe` | The Windows installer ([PACKAGING.md](PACKAGING.md#the-windows-installer)) |
| `…-linux-amd64.deb`, `…-linux-x86_64.rpm` | The Linux packages ([PACKAGING.md](PACKAGING.md#the-linux-packages)) |
| `…-licence-report-windows.json`, `…-licence-report-linux.json` | Every third-party component each platform's build ships, with the licence V2 uses it under (`pnpm notices` with `--report`, [LICENSING.md](LICENSING.md)) |
| `build-info.json` | The version, the tag if any, the commit, when and by which run it was built, each file's size and SHA-256, and each platform's native build records: the libcurl, OpenSSL, nghttp2, zlib, libtorrent and Boost versions with their sources, and the toolchains ([BUILDING_NATIVE.md](BUILDING_NATIVE.md#build-infojson)) |
| `SHA256SUMS` | The SHA-256 of every file above, in the `sha256sum` format |

It is kept **14 days** ([CI.md](CI.md#costs)). The run's summary repeats the checksums.

## Starting one

Only from a commit on `main` whose *Main checks* run on `main` passed. The workflow refuses any other: a commit on another branch, one without a green run, or a tag whose version isn't `package.json`'s. The rules are in [`scripts/release-candidate-lib.mjs`](../scripts/release-candidate-lib.mjs).

- **By hand,** for a test build of the latest `main` or one of its commits (the candidate is named `main`):

  ```sh
  gh workflow run release-candidate.yml --ref main -f commit=<sha, or main>
  ```

- **From a tag,** for a numbered candidate: the PO pushes `vX.Y.Z-rc.N` on a verified `main` commit (or tells an agent to, for that tag). Tags are never moved; a failed candidate is followed by `-rc.N+1` ([DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md#releases)).

**What runs:** the same packaging workflows as by hand, from exactly the verified commit, with their own checks: the Windows installer's contents, the Linux packages' contents, libraries and glibc, and their install, check and removal in Ubuntu 22.04, Ubuntu 24.04, Debian 12 and Fedora 42 containers. The tests themselves (L1 to L3) are the green *Main checks* run the commit needed. Every job runs only when every job it needs succeeded, which a test checks for this workflow and both packaging workflows.

## Checking a download

- **Linux:** `sha256sum --check --ignore-missing SHA256SUMS` in the folder with the files.
- **Windows:** `Get-FileHash <file>` in PowerShell, compared with the file's line in `SHA256SUMS`.
- **Which commit:** `build-info.json`'s `commit`, and the run linked in its `run`.

## Installing it beside V1

The installers are V2's release identity, so they install beside V1 and never touch it ([PACKAGING.md](PACKAGING.md), [COEXISTENCE.md](COEXISTENCE.md)). On Windows, run the installer; on Linux, `sudo apt install ./<file>.deb` or `sudo dnf install ./<file>.rpm`.

- **Windows SmartScreen** warns, because the installer isn't signed: **More info**, then **Run anyway**.
- **The PO's test** for #17 and #81: install the candidate on the Windows PC with V1 installed, run both, then upgrade it with a later candidate, and record the result on the issue ([PACKAGING.md](PACKAGING.md#testing-it-beside-v1)).

## Going back to an earlier version

A newer V2 may have changed the format of its data, and an older one can't always read it: the download list then says the saved downloads are from a newer Velocity. So:

1. **Before installing a candidate,** copy V2's data folder: `%APPDATA%\com.velocity.downloadmanager.v2` on Windows, `~/.local/share/com.velocity.downloadmanager.v2` on Linux. Quit V2 first.
2. **To go back:**
   - **Windows:** uninstall V2 from Apps & features without ticking "Delete the application data", then run the earlier installer.
   - **Linux:** `sudo apt install --allow-downgrades ./<earlier>.deb`, or `sudo dnf downgrade ./<earlier>.rpm`.
3. **If the earlier version can't read the data,** quit it, put the copy from step 1 back in place of the data folder, and start it again.

Downloaded files are never touched by installing, upgrading or going back.
