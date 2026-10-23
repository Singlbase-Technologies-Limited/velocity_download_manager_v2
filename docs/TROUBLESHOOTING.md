# Troubleshooting

## Setup

Start with `pnpm check:prereqs`. It names each missing prerequisite and prints the fix.

| Symptom | Cause | Fix |
|---|---|---|
| `pnpm setup` changes your shell profile instead of checking prerequisites | `setup` is a built-in pnpm command | Use `pnpm check:prereqs` |
| `pnpm: command not found`, or the wrong pnpm version | pnpm is missing, or Corepack isn't enabled | `corepack enable`, or `npx get-pnpm`; the pinned version is in `package.json` → `packageManager` |
| `cargo: command not found` just after installing Rust | The terminal was opened before rustup updated `PATH` | Open a new terminal. `pnpm check:prereqs` also looks in `~/.cargo/bin` |
| `link.exe` not found or `LNK1181` on Windows | Microsoft C++ Build Tools are missing, or the Rust toolchain is GNU instead of MSVC | Install the "Desktop development with C++" workload; check that `rustup show` lists `x86_64-pc-windows-msvc` |
| `webkit2gtk-4.1` not found on Linux | WebKitGTK development files are missing | Install the apt packages in [DEVELOPMENT.md](DEVELOPMENT.md#linux-debianubuntu) |
| `Port 1430 is in use` | Another V2 dev server is already running | Stop it. V1's dev server uses port 1420, so it doesn't conflict |
| Every file shows as modified after cloning on Windows | Line endings were converted | `.gitattributes` stores text with LF; run `git add --renormalize .` and check that you aren't overriding `core.autocrlf` per repository |
| The first `pnpm dev` seems stuck | It is compiling every Rust dependency (about 4–5 minutes on the reference machine) | Wait; later runs take seconds |
| The window heading shows "Could not reach the native shell" | The UI was opened in a browser (`pnpm dev:web`), where Tauri commands don't exist | Use `pnpm dev` |

## Downloads

These are the messages the engine reports when a download waits or fails. The app shows them once the downloads list is built (#59). What the engine does in each case is in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#retries-waits-and-stalls-48).

| Message or status | Cause | What to do |
|---|---|---|
| Retrying, with "Retrying automatically (1 of 2)." | The download failed in a way worth retrying, and the engine waits before starting it again | Nothing. If it ends as Failed, the message says why |
| Stalled | No data has come from the server for 15 seconds | Nothing at first: after 30 seconds the connection is reset and tried again. If it keeps stalling, check the network connection |
| "The server is rate limiting requests (429)." or "The server is temporarily unavailable or overloaded (503)." | The server asked clients to slow down | Velocity waits as long as the server asks, up to 10 minutes, and uses fewer connections. If it still fails, try again later |
| "The file was not found on the server (404). The link may have expired." | The address is wrong or no longer valid | Get a new link from the site |
| "Access was denied by the server (403)." or "The server requires authentication (401)." | The link needs a login, or has expired | Get a new link, or sign in on the site. Logins for downloads come with #70 |
| "The server name could not be resolved." | No network, or the name doesn't exist | Check the network connection and the address. The engine retries it automatically first |
| "The server's certificate isn't trusted, so the connection was refused." | The site's TLS certificate failed verification. Verification is never turned off | Check the computer's date and time, and whether a proxy or security program intercepts connections. Otherwise the site has to fix its certificate |
| "There is not enough disk space." | The disk filled up during the download | Free some space, then retry |
| "the transfer engine stopped responding and was restarted" | The engine hung and was restarted | Retry the download. If it happens again, report it with the engine's log |
