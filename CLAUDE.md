# Claude Code instructions

@AGENTS.md

AGENTS.md above holds the rules for everyone. This file adds how Claude Code works through an issue in this repository.

## Instructions versus enforcement

These files are guidance. `.claude/settings.json` adds a few deny and ask rules that block the most damaging git and GitHub commands outright. Those rules match command text only, so they are a safety net, not a security boundary. The rules stand whether a command is blocked or not.

## Working on an issue

1. **Investigate.**
   - Read the issue, its dependencies, its linked documents and its comments; decisions are often recorded there.
   - Check the live state before acting: `git status`, the current branch, `main` compared with `origin/main`, and open pull requests.
   - Trust the repository over summaries and memory.
2. **Plan.**
   - For work that touches several files or changes the architecture, write a short plan before editing.
   - Search for existing code that already does the job before planning new code (AGENTS.md, "Reuse before you add").
   - Decisions that belong to the PO are asked, not assumed. Ask with multiple-choice questions in plain words.
3. **Branch.** Create `<type>/<issue>-<slug>` from the latest `main`, and set the issue to *In progress* on the project board.
4. **Test first.**
   - For meaningful behaviour, run the new test and see it fail before writing the code.
   - Iterate with `pnpm test:focused -- <area>`, then run `pnpm check:fast` before the pull request.
5. **When something fails, stop.** Report the actual error. Don't weaken a test, a lint, a check or a security control to get past it.
6. **Pull request.**
   - Check the identity first ([docs/AI_AGENT_ACCOUNT.md](docs/AI_AGENT_ACCOUNT.md)).
   - Run `gh` for this repository as `GH_TOKEN="$(gh auth token -u velocity-claude-agent)" gh …`. Never run `gh auth switch`.
   - The PR description lists what changed, the evidence for each acceptance criterion, and every check as PASS, FAIL, BLOCKED or NOT RUN.
   - Set the issue to *In review*.
7. **Merge only on the PO's explicit approval of that PR,** with `gh pr merge <N> --squash --delete-branch`. Then update local `main`, check that the issue's state is right, and report.

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
