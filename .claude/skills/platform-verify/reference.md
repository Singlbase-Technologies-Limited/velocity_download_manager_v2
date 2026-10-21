# Platform verification reference

These are the machine-specific details for `/platform-verify`, as set up on the PO's PC (2026-10-21). If a path or version below no longer matches the machine, update this file in the same PR.

## WSL environment

- **Distribution:** Ubuntu 24.04 under WSL2. It has 2 vCPUs and about 1.9 GiB of RAM, so long Rust links can swap and their timings vary a lot.
- **Tools are installed per user,** not system-wide:
  - Rust: `~/.cargo/bin` (toolchain from `rust-toolchain.toml`).
  - Node 24.12.0: `~/.local/node-v24.12.0-linux-x64/bin`; pnpm through `corepack pnpm`.
- **Every non-interactive command needs** `export PATH=$HOME/.cargo/bin:$HOME/.local/node-v24.12.0-linux-x64/bin:$PATH`.

## Run commands through a script file

PowerShell strips the inner double quotes from `wsl -e bash -lc "…"`, which breaks `grep -E "(a|b)"` and `$(…)`. Write the commands to a script in the session's scratch directory instead, convert its path, and run it:

```sh
W=$(wsl -e wslpath -a "$(cygpath -m "$SCRATCH")/step.sh" | tr -d '\r')
MSYS_NO_PATHCONV=1 wsl -e bash "$W"
```

From Git Bash, `MSYS_NO_PATHCONV=1` stops Git Bash from rewriting the Linux path.

## Update the clean clone

The clean clone lives at `~/v2-linux-check`. Fetch the branch straight from the Windows working tree, so nothing has to be pushed first:

```sh
cd ~/v2-linux-check
git fetch -q /mnt/c/projects/velocity_download_manager_v2 <branch>
git switch -q -C <branch> FETCH_HEAD
git status --short   # must be empty: the clone is only for checks
```

If the clone is missing, clone it again from GitHub through the agent's SSH alias, or from `/mnt/c/projects/velocity_download_manager_v2`. Never edit files in it.

## GUI through WSLg

Start GUI apps from a script file, never from inline PowerShell arguments. Run `pnpm dev` in the background, log to a file in `/tmp`, and stop it afterwards. A screenshot for docs or a PR shows the app window only.

## Recording results

`docs/DEVELOPMENT.md` lists the platforms with their environments, and has the timing tables, including "Check timings". Add a dated row instead of overwriting older results, unless the old result is wrong.
