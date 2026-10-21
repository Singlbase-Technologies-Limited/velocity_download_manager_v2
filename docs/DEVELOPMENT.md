# Development setup

This page takes you from a fresh clone to a running Velocity Download Manager V2 window on Windows, Linux or macOS. If something fails, see [TROUBLESHOOTING.md](TROUBLESHOOTING.md).

## Quick start

After installing the prerequisites below:

```sh
pnpm check:prereqs            # checks Node.js, pnpm, Rust and the platform libraries; prints the fix for anything missing
pnpm install --frozen-lockfile
pnpm dev                      # builds the Rust shell and opens the app with hot reload
```

The first `pnpm dev` compiles every Rust dependency and takes several minutes (see [Build times](#build-times)). Later runs reuse the build.

You should see a window titled **Velocity Download Manager V2 (dev)** whose heading shows the name and version reported by the Rust core:

![The V2 development shell on Windows](images/dev-shell-windows.png)

## Prerequisites

| Tool | Version | Why |
|---|---|---|
| Node.js | `^22.13.0 \|\| ^24.0.0 \|\| >=26.0.0` (the `engines` field in `package.json`; set by Vitest and ESLint) | Frontend tooling |
| pnpm | exactly the version in `package.json` → `packageManager` (12.14.0) | Package manager |
| Rust | installed with rustup; `rust-toolchain.toml` pins the toolchain (1.99.1) and rustup installs it automatically | Desktop shell and core |
| Platform libraries | see below | Tauri's webview and native build |

`pnpm check:prereqs` checks all of these. It only reads your system; it never installs or changes anything.

### pnpm

Use one of the routes from the [pnpm installation docs](https://pnpm.io/installation):

- **Corepack** (bundled with the Node.js releases we support): run `corepack enable` once. Corepack then runs the exact pnpm version pinned in `package.json`.
- **npm**: `npx get-pnpm` (needs Node.js 22.13 or newer). pnpm's docs recommend this route on Windows, because Windows Defender may block the standalone installer.

Note that `pnpm setup` and `pnpm doctor` are built-in pnpm commands, which is why the prerequisite check is named `check:prereqs`.

### Windows 10/11

Requirements from the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/):

1. **Microsoft C++ Build Tools** with the **Desktop development with C++** workload: <https://visualstudio.microsoft.com/visual-cpp-build-tools/>.
2. **WebView2 Runtime**. It is preinstalled on Windows 10 (1803 and later) and Windows 11; otherwise install the Evergreen Bootstrapper from <https://developer.microsoft.com/microsoft-edge/webview2/>.
3. **Rust**: `winget install --id Rustlang.Rustup`, and choose the MSVC toolchain (`x86_64-pc-windows-msvc`) as the default host. Open a new terminal afterwards.
4. **Node.js** LTS from <https://nodejs.org/>, then pnpm as described above.

### Linux (Debian/Ubuntu)

```sh
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh
```

Then install Node.js and pnpm. Fedora and Arch package lists are in the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/#linux).

### macOS

```sh
xcode-select --install
curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh
```

Then install Node.js and pnpm.

## Everyday commands

| Command | What it does |
|---|---|
| `pnpm dev` | Run the desktop app with hot reload (Vite on port 1430, Rust rebuilt on change) |
| `pnpm dev:web` | Run only the web UI in a browser (native commands are unavailable there) |
| `pnpm check:fast` | Level L1 checks: `cargo fmt --check`, clippy with `-D warnings`, `cargo test`, `tsc`, ESLint (zero warnings), Vitest. It runs every check even after a failure and prints a summary |
| `pnpm test` | Frontend and script unit tests (Vitest) |
| `pnpm rust:test` | Rust unit tests |
| `pnpm typecheck`, `pnpm lint` | TypeScript and ESLint on their own |

Run `pnpm check:fast` before every pull request. The other verification levels (L2–L5) are defined in issue #15.

## Running beside the original Velocity Download Manager

V2 is a separate application. During development it uses placeholder identifiers until the identity decision (issue #11) is recorded:

| | V1 | V2 (development placeholder) |
|---|---|---|
| Product name | Velocity Download Manager | Velocity Download Manager V2 (dev) |
| Bundle identifier | `com.velocity.downloadmanager` | `dev.velocity.downloadmanager.v2` |
| Dev server port | 1420 | 1430 |

The identifiers differ, so the two apps don't share data directories or single-instance locks. The ports differ, so both dev servers can run at once.

## Build times

Measured 2026-10-21 on Windows 11 (build 26200), Intel Core i7-13620H (16 logical CPUs), 15.7 GiB RAM, NVMe SSD, Rust 1.99.1, Node.js 24.12.0, pnpm 12.14.0:

| Step | Time |
|---|---|
| First `cargo build --workspace` from an empty `target/` | 278 s |
| `pnpm check:fast`, first run (clippy compiles its own artefacts) | 128 s |
| `pnpm check:fast`, warm | 7 s |
| `cargo build --workspace` with no changes | 3 s |
| Rebuild after a one-line change in `crates/velocity-core` | 19 s |
| Rebuild after a one-line change in `src-tauri` | 17 s |
| Vite dev server start | 0.23 s |

Linking the desktop executable dominates incremental rebuilds. Issue #3 tunes this.

| Platform | Clean-clone run |
|---|---|
| Windows 11 | **PASS** (2026-10-21, machine above) |
| Linux (Ubuntu 24.04) | **NOT RUN**: no Linux machine with the prerequisites installed was available |
| macOS | **NOT RUN**: no macOS machine was available |

## Repository layout

| Path | Purpose |
|---|---|
| `src/` | React and TypeScript UI. It talks to Rust only through typed Tauri commands |
| `src-tauri/` | Tauri 2 desktop shell. Commands stay thin and delegate to the core |
| `crates/velocity-core/` | Core domain logic in Rust |
| `scripts/` | Developer scripts (`check:prereqs`, `check:fast`) and their tests |
| `docs/` | Project documentation |
