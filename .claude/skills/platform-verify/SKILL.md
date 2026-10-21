---
name: platform-verify
description: Verify the current branch on Windows (native) and Linux (WSL2 clean clone), with macOS reported as NOT RUN unless a Mac is available, and record the evidence in the platform format used in docs/DEVELOPMENT.md. Manual; use for build, packaging, filesystem, process or UI changes that can differ by platform.
disable-model-invocation: true
argument-hint: "[windows|linux|all]"
---

# Platform verification

Platforms requested: `$ARGUMENTS`. Empty means all. Run on each platform that applies: `windows`, `linux` or `all`. The machine-specific setup, including paths, the WSL quirks and the GUI launch, is in [reference.md](reference.md). Read it before the Linux steps.

## Windows (native, the PO's PC)

1. **Checks:** in the repository on the branch, run `pnpm check:fast`, then the level the change needs (`/verify-v2`).
2. **Runtime behaviour:** if the change affects it, run `pnpm dev` and confirm the behaviour. A screenshot for docs or the PR is cropped to the app window only, never the desktop.

## Linux (WSL2 Ubuntu, clean clone)

1. **Update the clean clone** at `~/v2-linux-check` to this branch: fetch from the Windows working tree, so nothing needs pushing first ([reference.md](reference.md#update-the-clean-clone)).
2. **Install and check:** `pnpm install --frozen-lockfile`, then `pnpm check:fast`, plus `pnpm check:full` when the change touches build settings.
3. **GUI changes:** launch the app through WSLg with a script file ([reference.md](reference.md#gui-through-wslg)).

## macOS

**NOT RUN** unless a Mac is available. The PO accepted this for #2. Never infer a macOS result from Linux.

## Report

| Platform | Environment | Commands | Result |
|---|---|---|---|
| Windows 11 | build, CPU, RAM; Rust, Node and pnpm versions | exact commands | PASS / FAIL / BLOCKED / NOT RUN + key output |
| Linux (WSL2 Ubuntu 24.04) | kernel, vCPU, RAM | … | … |
| macOS | — | — | NOT RUN: no Mac |

**When results go into docs** (for example timings in `docs/DEVELOPMENT.md`), give the date, the machine and the run count. Note that WSL2 is a real Linux kernel, but not a native desktop install.
