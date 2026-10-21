# Claude Code instructions

@AGENTS.md

AGENTS.md above holds the rules for everyone. This file adds how Claude Code works through an issue in this repository.

## Instructions versus enforcement

These files are guidance. `.claude/settings.json` adds a few deny and ask rules that block the most damaging git and GitHub commands outright. Those rules match command text only, so they are a safety net, not a security boundary. The rules stand whether a command is blocked or not.

## Working on an issue

The skills in the table below hold the details of each step.

1. **Investigate and plan** (`/investigate-issue <N>`).
   - Read the issue, its comments and dependencies, and check the live state. Trust the repository over summaries and memory.
   - Search for existing code to reuse.
   - Plan multi-file or architecture work before editing.
   - PO decisions are asked, not assumed: use multiple-choice questions in plain words.
2. **Branch.** Create `<type>/<issue>-<slug>` from the latest `main`, and set the issue to *In progress* on the project board.
3. **Test first** (`/tdd-change`). Red, green, refactor, one behaviour at a time, with `pnpm test:focused -- <area>`.
4. **When something fails, stop.** Report the actual error. Don't weaken a test, a lint, a check or a security control to get past it.
5. **Verify** (`/verify-v2`). Add `/platform-verify` and `/security-check` when the change calls for them.
6. **Pull request** (`/prepare-pr <N>`).
   - Run `gh` for this repository as `GH_TOKEN="$(gh auth token -u velocity-claude-agent)" gh …`. Never run `gh auth switch`.
   - Set the issue to *In review*.
7. **Merge only on the PO's explicit approval of that PR,** with `gh pr merge <N> --squash --delete-branch`. It prompts once more by design. Then update local `main`, check that the issue's state is right, and report.

## Skills

| Skill | Invoked by | Use it to |
|---|---|---|
| [`/investigate-issue <N>`](.claude/skills/investigate-issue/SKILL.md) | You or Claude | Start an issue: read everything, check the live state, find code to reuse, plan |
| [`/tdd-change <behaviour>`](.claude/skills/tdd-change/SKILL.md) | You or Claude | Implement one behaviour with Red–Green–Refactor and the right focused test command |
| [`/verify-v2 [full]`](.claude/skills/verify-v2/SKILL.md) | You only | Run the verification levels the change needs, and report each check |
| [`/platform-verify [windows\|linux\|all]`](.claude/skills/platform-verify/SKILL.md) | You only | Check Windows natively and Linux in the WSL clean clone; macOS is NOT RUN |
| [`/security-check`](.claude/skills/security-check/SKILL.md) | You only | V2-specific security review of the diff, alongside the built-in `/security-review` |
| [`/prepare-pr <N>`](.claude/skills/prepare-pr/SKILL.md) | You only | The pre-PR checklist and description draft; it stops before pushing |

Skills for benchmarks (#20), release candidates (#17, #84) and CI triage (#91) come with those issues, once the procedures they wrap exist.

## Path-scoped rules

When you read or edit a matching file, Claude Code loads the rule for that area. Read the rule before changing files in its area.

| Rule | Applies to |
|---|---|
| [rust-native.md](.claude/rules/rust-native.md) | Rust in `crates/` and `src-tauri/` |
| [transfer-engine.md](.claude/rules/transfer-engine.md) | The direct-download engine and the libcurl layer (paths pending #33) |
| [persistence.md](.claude/rules/persistence.md) | Database, migrations and checkpoints |
| [frontend.md](.claude/rules/frontend.md) | `src/` |
| [browser-integration.md](.claude/rules/browser-integration.md) | The browser extension and native host (paths pending #64, #65) |
| [security.md](.claude/rules/security.md) | Native code, scripts and configuration that cross a trust boundary |
| [testing.md](.claude/rules/testing.md) | Test files and fixtures |
| [ci-release.md](.claude/rules/ci-release.md) | CI workflows, scripts and packaging configuration |
| [documentation.md](.claude/rules/documentation.md) | Markdown documents |

## Tools and sessions

- **Built-in tools first:** prefer Claude Code's built-in tools and skills. Don't install plugins, MCP servers or third-party skills without a concrete need and PO approval.
- **Windows shells:**
  - Git Bash rewrites arguments that start with `/` into paths, so run `claude -p "/…"` from PowerShell.
  - In WSL, start GUI apps through a script file, not inline arguments.
- **Temporary files:** logs and other temporary output go to the session's scratch directory, never into the repository.
- **Screenshots:** for docs or PRs, crop them to the app window only.
- **Long runs:** don't run the full verification after every small edit. `pnpm check:pr` picks what a change needs.
