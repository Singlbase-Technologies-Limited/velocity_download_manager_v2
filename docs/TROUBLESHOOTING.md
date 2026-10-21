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
